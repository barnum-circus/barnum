//! The multiplexer daemon that manages agent pools.

use crate::constants::{AGENTS_DIR, LOCK_FILE, NEXT_TASK_FILE, OUTPUT_FILE, SOCKET_NAME};
use crate::lock::acquire_lock;
use interprocess::local_socket::{
    prelude::*, GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, Stream,
};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{fs, io};
use tracing::{debug, info, trace};

/// The multiplexer daemon that manages agents and dispatches tasks.
pub struct Multiplexer {
    agents_folder: PathBuf,
    lock_path: PathBuf,
    socket_path: PathBuf,
    agents: HashMap<String, AgentState>,
    pending_tasks: VecDeque<PendingTask>,
}

struct PendingTask {
    content: String,
    response_stream: Stream,
}

#[derive(Debug)]
enum AgentState {
    Available,
    Busy { response_stream: Stream },
}

impl Multiplexer {
    /// Create a new multiplexer for the given root folder.
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        let agents_folder = root.join(AGENTS_DIR);
        let lock_path = root.join(LOCK_FILE);
        let socket_path = root.join(SOCKET_NAME);

        fs::create_dir_all(&root)?;
        fs::create_dir_all(&agents_folder)?;

        acquire_lock(&lock_path)?;

        // Clean up stale socket file on Unix
        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }

        let mut multiplexer = Self {
            agents_folder,
            lock_path,
            socket_path,
            agents: HashMap::new(),
            pending_tasks: VecDeque::new(),
        };

        multiplexer.scan_existing_agents()?;

        Ok(multiplexer)
    }

    /// Run the multiplexer event loop.
    pub fn run(&mut self) -> io::Result<()> {
        let name = self
            .socket_path
            .clone()
            .to_fs_name::<GenericFilePath>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .map_err(|e| io::Error::new(io::ErrorKind::AddrInUse, e))?;

        listener
            .set_nonblocking(ListenerNonblockingMode::Accept)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let (fs_tx, fs_rx) = mpsc::channel();

        // Use a short poll interval for responsive agent detection
        let config = notify::Config::default()
            .with_poll_interval(Duration::from_millis(100));

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = fs_tx.send(event);
                }
            },
            config,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        watcher
            .watch(&self.agents_folder, RecursiveMode::Recursive)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        info!(socket = %self.socket_path.display(), "listening");

        self.event_loop(listener, fs_rx)
    }

    fn event_loop(
        &mut self,
        listener: Listener,
        fs_rx: mpsc::Receiver<Event>,
    ) -> io::Result<()> {
        let mut last_scan = std::time::Instant::now();
        let scan_interval = Duration::from_millis(200);

        loop {
            match listener.accept() {
                Ok(stream) => self.handle_submit(stream)?,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }

            while let Ok(event) = fs_rx.try_recv() {
                self.handle_fs_event(event)?;
            }

            // Periodic rescan to catch agents that file watching might miss
            // (e.g., due to FSEvents latency on macOS)
            if last_scan.elapsed() >= scan_interval {
                self.scan_existing_agents()?;
                last_scan = std::time::Instant::now();
            }

            self.try_dispatch_tasks()?;

            thread::sleep(Duration::from_millis(10));
        }
    }

    fn scan_existing_agents(&mut self) -> io::Result<()> {
        for entry in fs::read_dir(&self.agents_folder)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    self.register_agent(name);
                }
            }
        }
        Ok(())
    }

    fn register_agent(&mut self, agent_id: &str) {
        if self.agents.contains_key(agent_id) {
            return;
        }
        info!(agent_id, "agent registered");
        self.agents.insert(agent_id.to_string(), AgentState::Available);
    }

    fn unregister_agent(&mut self, agent_id: &str) {
        if self.agents.remove(agent_id).is_some() {
            info!(agent_id, "agent unregistered");
        }
    }

    fn handle_submit(&mut self, stream: Stream) -> io::Result<()> {
        let mut reader = BufReader::new(&stream);

        let mut len_line = String::new();
        reader.read_line(&mut len_line)?;

        let len: usize = match len_line.trim().parse() {
            Ok(n) => n,
            Err(_) => return Ok(()),
        };

        let mut content = vec![0u8; len];
        reader.read_exact(&mut content)?;

        let content = match String::from_utf8(content) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };

        info!(
            bytes = content.len(),
            pending = self.pending_tasks.len(),
            agents = self.agents.len(),
            "task received"
        );
        self.pending_tasks.push_back(PendingTask {
            content,
            response_stream: stream,
        });

        Ok(())
    }

    fn handle_fs_event(&mut self, event: Event) -> io::Result<()> {
        trace!(kind = ?event.kind, paths = ?event.paths, "fs event");

        for path in &event.paths {
            if !path.starts_with(&self.agents_folder) {
                trace!(path = %path.display(), "skipping path outside agents folder");
                continue;
            }

            let Some(relative) = path.strip_prefix(&self.agents_folder).ok() else {
                continue;
            };

            let components: Vec<_> = relative.components().collect();
            if components.is_empty() {
                continue;
            }

            let Some(agent_id) = components[0].as_os_str().to_str() else {
                continue;
            };

            if agent_id.is_empty() {
                continue;
            }

            debug!(agent_id, components = components.len(), "processing fs event");

            match components.len() {
                1 => self.handle_agent_folder_event(&event, agent_id),
                2 => {
                    let Some(filename) = components[1].as_os_str().to_str() else {
                        continue;
                    };
                    self.handle_agent_file_event(&event, agent_id, filename, path)?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn handle_agent_folder_event(&mut self, event: &Event, agent_id: &str) {
        let agent_folder = self.agents_folder.join(agent_id);
        let is_dir = agent_folder.is_dir();

        debug!(agent_id, kind = ?event.kind, is_dir, "folder event");

        if matches!(event.kind, EventKind::Remove(_)) {
            self.unregister_agent(agent_id);
        } else if is_dir {
            // Register on any event if folder exists (Create, Modify, etc.)
            self.register_agent(agent_id);
        }
    }

    fn handle_agent_file_event(
        &mut self,
        event: &Event,
        agent_id: &str,
        filename: &str,
        path: &Path,
    ) -> io::Result<()> {
        // Any file event in an agent folder means the agent exists
        let agent_folder = self.agents_folder.join(agent_id);
        if agent_folder.is_dir() {
            self.register_agent(agent_id);
        }

        if filename == OUTPUT_FILE
            && matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
            && path.exists()
        {
            self.handle_agent_output(agent_id, path)?;
        }

        Ok(())
    }

    fn handle_agent_output(&mut self, agent_id: &str, output_path: &Path) -> io::Result<()> {
        let response_stream = match self.agents.remove(agent_id) {
            Some(AgentState::Busy { response_stream }) => response_stream,
            other => {
                if let Some(state) = other {
                    self.agents.insert(agent_id.to_string(), state);
                }
                return Ok(());
            }
        };

        let output = match fs::read_to_string(output_path) {
            Ok(o) => o,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.agents
                    .insert(agent_id.to_string(), AgentState::Busy { response_stream });
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // Clean up both files - output and next_task
        let _ = fs::remove_file(output_path);
        let task_path = self.agents_folder.join(agent_id).join(NEXT_TASK_FILE);
        let _ = fs::remove_file(task_path);

        self.send_response(response_stream, &output)?;

        info!(agent_id, "task completed");
        self.agents.insert(agent_id.to_string(), AgentState::Available);

        Ok(())
    }

    fn send_response(&self, mut stream: Stream, output: &str) -> io::Result<()> {
        writeln!(stream, "{}", output.len())?;
        stream.write_all(output.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    fn try_dispatch_tasks(&mut self) -> io::Result<()> {
        loop {
            let available_agent = self
                .agents
                .iter()
                .find(|(_, state)| matches!(state, AgentState::Available))
                .map(|(id, _)| id.clone());

            let Some(agent_id) = available_agent else {
                break;
            };

            let Some(task) = self.pending_tasks.pop_front() else {
                break;
            };

            let task_path = self.agents_folder.join(&agent_id).join(NEXT_TASK_FILE);
            fs::write(&task_path, &task.content)?;

            info!(agent_id, "task dispatched");
            self.agents.insert(
                agent_id,
                AgentState::Busy {
                    response_stream: task.response_stream,
                },
            );
        }

        Ok(())
    }
}

impl Drop for Multiplexer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
        let _ = fs::remove_file(&self.socket_path);
    }
}
