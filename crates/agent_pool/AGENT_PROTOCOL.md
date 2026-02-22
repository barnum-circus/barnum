# Agent Protocol

How to be an agent in the pool.

## Setup

Create your agent directory:

```
mkdir -p /path/to/pool/agents/my-agent
```

## The Protocol

Tasks use numbered files: `{id}.input` and `{id}.output`.

**When you see a `.input` file:**

1. Read it
2. Do the work
3. Check if the `.input` file still exists:
   - **If yes**: Write `{id}.output`, then delete `{id}.input`
   - **If no**: Task was timed out, don't write output

The pool cleans up both files after reading your output.

## Example

```bash
AGENT_DIR="/path/to/pool/agents/my-agent"

while true; do
    # Find the input file (there's only ever one)
    input_file=$(ls "$AGENT_DIR"/*.input 2>/dev/null | head -1)

    if [ -n "$input_file" ]; then
        task=$(cat "$input_file")

        # Do your work here...
        result="done"

        # Check if we were timed out
        if [ -f "$input_file" ]; then
            # Get the task ID from filename (e.g., "1.input" -> "1")
            id=$(basename "$input_file" .input)
            echo "$result" > "$AGENT_DIR/$id.output"
            rm -f "$input_file"
        fi
    fi
    sleep 0.1
done
```

## For Claude Code

If you're a Claude Code instance acting as an agent:

1. Create your directory: `mkdir -p /path/to/pool/agents/YOUR_NAME`
2. Watch for `*.input` files
3. Read the input, do the work
4. Before writing output, verify the input file still exists (if not, you were timed out)
5. Write `{id}.output` and delete `{id}.input`
