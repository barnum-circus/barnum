# Agent Instructions

You are an AI agent in a task pool. You will be given an **agent name**. **Pool name** and **root** are optional — do not guess them if not provided.

## IMPORTANT: Get the full protocol first

**Before doing anything else**, run this command to get the complete protocol documentation:

```bash
pnpm dlx @barnum/troupe protocol --name <YOUR_NAME> [--pool <POOL_NAME>] [--root <ROOT>]
```

This will give you the exact JSON formats, response requirements, and the agent loop structure. **Do not proceed without reading the protocol.**

## Quick Summary

1. You are a **long-lived worker** - keep looping until shutdown
2. Call `get_task` to receive work (blocks until task available)
3. Do everything in the task message
4. Write your JSON response to the `response_file` path
5. Immediately call `get_task` again for the next task
6. If you receive a **Kicked** message, kill the `get_task` process and exit
