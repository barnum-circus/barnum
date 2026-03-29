import React, { useState, useEffect, useCallback } from "react";

type Todo = {
  id: number;
  text: string;
  completed: boolean;
  priority: "low" | "medium" | "high";
  createdAt: string;
};

// Manages a todo list with filtering.
// Anti-patterns: useEffect to sync filtered results, prop drilling,
// unnecessary useCallback, over-use of useState for derived values.
export function TodoList() {
  const [todos, setTodos] = useState<Todo[]>([]);
  const [filter, setFilter] = useState<"all" | "active" | "completed">("all");
  const [priorityFilter, setPriorityFilter] = useState<string>("all");
  const [searchQuery, setSearchQuery] = useState("");

  // Derived state stored in useState and synced via useEffect
  const [filteredTodos, setFilteredTodos] = useState<Todo[]>([]);
  const [todoCount, setTodoCount] = useState(0);
  const [completedCount, setCompletedCount] = useState(0);
  const [activeCount, setActiveCount] = useState(0);
  const [highPriorityCount, setHighPriorityCount] = useState(0);

  // Chain of useEffects to compute derived state
  useEffect(() => {
    let result = [...todos];
    if (filter === "active") {
      result = result.filter((t) => !t.completed);
    } else if (filter === "completed") {
      result = result.filter((t) => t.completed);
    }
    if (priorityFilter !== "all") {
      result = result.filter((t) => t.priority === priorityFilter);
    }
    if (searchQuery) {
      result = result.filter((t) =>
        t.text.toLowerCase().includes(searchQuery.toLowerCase())
      );
    }
    setFilteredTodos(result);
  }, [todos, filter, priorityFilter, searchQuery]);

  useEffect(() => {
    setTodoCount(todos.length);
  }, [todos]);

  useEffect(() => {
    setCompletedCount(todos.filter((t) => t.completed).length);
  }, [todos]);

  useEffect(() => {
    setActiveCount(todos.filter((t) => !t.completed).length);
  }, [todos]);

  useEffect(() => {
    setHighPriorityCount(todos.filter((t) => t.priority === "high").length);
  }, [todos]);

  // Load initial todos
  useEffect(() => {
    fetch("/api/todos")
      .then((res) => res.json())
      .then(setTodos);
  }, []);

  // Unnecessary useCallback — these are simple setState calls
  const handleToggle = useCallback(
    (id: number) => {
      setTodos(
        todos.map((t) => (t.id === id ? { ...t, completed: !t.completed } : t))
      );
    },
    [todos]
  );

  const handleDelete = useCallback(
    (id: number) => {
      setTodos(todos.filter((t) => t.id !== id));
    },
    [todos]
  );

  const handleAdd = useCallback(
    (text: string, priority: "low" | "medium" | "high") => {
      const newTodo: Todo = {
        id: Date.now(),
        text,
        completed: false,
        priority,
        createdAt: new Date().toISOString(),
      };
      setTodos([...todos, newTodo]);
    },
    [todos]
  );

  return (
    <div style={{ maxWidth: "600px", margin: "0 auto" }}>
      <h1>Todo List</h1>
      <div style={{ marginBottom: "16px" }}>
        <span>Total: {todoCount}</span>
        <span style={{ marginLeft: "12px" }}>Active: {activeCount}</span>
        <span style={{ marginLeft: "12px" }}>Done: {completedCount}</span>
        <span style={{ marginLeft: "12px" }}>
          High Priority: {highPriorityCount}
        </span>
      </div>

      <AddTodoForm onAdd={handleAdd} />

      <div style={{ display: "flex", gap: "8px", marginBottom: "16px" }}>
        <input
          type="text"
          placeholder="Search..."
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          style={{ padding: "4px 8px" }}
        />
        <select
          value={filter}
          onChange={(e) => setFilter(e.target.value as any)}
        >
          <option value="all">All</option>
          <option value="active">Active</option>
          <option value="completed">Completed</option>
        </select>
        <select
          value={priorityFilter}
          onChange={(e) => setPriorityFilter(e.target.value)}
        >
          <option value="all">All Priorities</option>
          <option value="low">Low</option>
          <option value="medium">Medium</option>
          <option value="high">High</option>
        </select>
      </div>

      {filteredTodos.map((todo) => (
        <TodoItem
          key={todo.id}
          id={todo.id}
          text={todo.text}
          completed={todo.completed}
          priority={todo.priority}
          createdAt={todo.createdAt}
          onToggle={handleToggle}
          onDelete={handleDelete}
        />
      ))}
    </div>
  );
}

// Prop drilling: passes every field as a separate prop instead of the object
function TodoItem({
  id,
  text,
  completed,
  priority,
  createdAt,
  onToggle,
  onDelete,
}: {
  id: number;
  text: string;
  completed: boolean;
  priority: string;
  createdAt: string;
  onToggle: (id: number) => void;
  onDelete: (id: number) => void;
}) {
  // Unnecessary useState + useEffect for formatting a date
  const [formattedDate, setFormattedDate] = useState("");
  useEffect(() => {
    setFormattedDate(new Date(createdAt).toLocaleDateString());
  }, [createdAt]);

  // Unnecessary useState + useEffect for priority color
  const [priorityColor, setPriorityColor] = useState("");
  useEffect(() => {
    if (priority === "high") setPriorityColor("#ef4444");
    else if (priority === "medium") setPriorityColor("#f59e0b");
    else setPriorityColor("#22c55e");
  }, [priority]);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        padding: "8px",
        borderBottom: "1px solid #eee",
        opacity: completed ? 0.5 : 1,
      }}
    >
      <input
        type="checkbox"
        checked={completed}
        onChange={() => onToggle(id)}
        style={{ marginRight: "8px" }}
      />
      <span
        style={{
          flex: 1,
          textDecoration: completed ? "line-through" : "none",
        }}
      >
        {text}
      </span>
      <span
        style={{
          color: priorityColor,
          fontSize: "12px",
          marginRight: "8px",
        }}
      >
        {priority}
      </span>
      <span style={{ color: "#999", fontSize: "12px", marginRight: "8px" }}>
        {formattedDate}
      </span>
      <button onClick={() => onDelete(id)} style={{ color: "red" }}>
        ×
      </button>
    </div>
  );
}

function AddTodoForm({
  onAdd,
}: {
  onAdd: (text: string, priority: "low" | "medium" | "high") => void;
}) {
  const [text, setText] = useState("");
  const [priority, setPriority] = useState<"low" | "medium" | "high">("medium");

  // Unnecessary: tracks whether form is "valid" via useEffect
  const [isValid, setIsValid] = useState(false);
  useEffect(() => {
    setIsValid(text.trim().length > 0);
  }, [text]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!isValid) return;
    onAdd(text.trim(), priority);
    setText("");
  };

  return (
    <form
      onSubmit={handleSubmit}
      style={{ display: "flex", gap: "8px", marginBottom: "16px" }}
    >
      <input
        type="text"
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="Add a todo..."
        style={{ flex: 1, padding: "4px 8px" }}
      />
      <select
        value={priority}
        onChange={(e) => setPriority(e.target.value as any)}
      >
        <option value="low">Low</option>
        <option value="medium">Medium</option>
        <option value="high">High</option>
      </select>
      <button type="submit" disabled={!isValid}>
        Add
      </button>
    </form>
  );
}
