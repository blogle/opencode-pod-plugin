#!/usr/bin/env python3
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


COMMANDS = {
    "E2E_STOCK_TOOL": "printf 'created-by-stock-bash\\n' > stock-tool.txt",
    "E2E_PLUGIN": "printf '%s\\n' \"$FIXTURE_PLUGIN_LOADED\" > plugin-observed.txt",
    "E2E_RUNTIME": "test -x /opt/opencode/bin/opencode && test -x /opt/opencode/bin/bun && test -x /opt/opencode/bin/direnv && ! command -v opencode && ! command -v bun && ! command -v direnv && printf 'runtime-injected\\n' > runtime-observed.txt",
    "E2E_ENV": "printf '%s\\n' \"$FIXTURE_PRIVATE\" > env-observed.txt",
    "E2E_CHECKPOINT": "printf 'staged-change\\n' >> staged.txt && git add staged.txt && printf 'unstaged-change\\n' >> README.md && printf 'index-change\\n' >> both.txt && git add both.txt && printf 'worktree-change\\n' >> both.txt && printf 'untracked\\n' > untracked.txt && printf 'staged-new\\n' > staged-new.txt && git add staged-new.txt && git rm delete.txt",
    "E2E_PREVIEW": "nohup python3 server.py 18080 >/tmp/fixture-preview.log 2>&1 &",
    "E2E_ISOLATION_A": "printf 'workspace-a-only\\n' > isolation-a.txt",
    "E2E_CONTINUE": "test -f untracked.txt && printf 'continued-session\\n' > continued.txt",
}


def message_text(messages):
    values = []
    for message in messages:
        content = message.get("content", "")
        if isinstance(content, str):
            values.append(content)
        elif isinstance(content, list):
            for part in content:
                if isinstance(part, dict) and isinstance(part.get("text"), str):
                    values.append(part["text"])
    return "\n".join(values)


def latest_user_text(messages):
    for message in reversed(messages):
        if message.get("role") == "user":
            return message_text([message])
    return ""


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path in ("/healthz", "/v1/models"):
            payload = {"object": "list", "data": [{"id": "fixture-model", "object": "model"}]}
            self.json_response(payload)
            return
        self.send_error(404)

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", "0"))
        request = json.loads(self.rfile.read(length))
        messages = request.get("messages", [])
        latest_user = max(
            (index for index, message in enumerate(messages) if message.get("role") == "user"),
            default=-1,
        )
        has_tool_result = any(
            message.get("role") == "tool" for message in messages[latest_user + 1 :]
        )
        text = latest_user_text(messages)
        command = next((value for marker, value in COMMANDS.items() if marker in text), COMMANDS["E2E_STOCK_TOOL"])
        if has_tool_result:
            response = {"role": "assistant", "content": "fixture-turn-complete"}
            finish = "stop"
        else:
            response = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call_fixture_0001",
                        "type": "function",
                        "function": {
                            "name": "bash",
                            "arguments": json.dumps({"command": command, "description": "Run deterministic acceptance action"}),
                        },
                    }
                ],
            }
            finish = "tool_calls"
        if request.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "close")
            self.end_headers()
            chunk = {
                "id": "chatcmpl-fixture",
                "object": "chat.completion.chunk",
                "created": 1700000000,
                "model": "fixture-model",
                "choices": [{"index": 0, "delta": response, "finish_reason": None}],
            }
            self.wfile.write(("data: " + json.dumps(chunk, separators=(",", ":")) + "\n\n").encode())
            chunk["choices"][0] = {"index": 0, "delta": {}, "finish_reason": finish}
            chunk["usage"] = {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
            self.wfile.write(("data: " + json.dumps(chunk, separators=(",", ":")) + "\n\ndata: [DONE]\n\n").encode())
            self.wfile.flush()
            return
        payload = {
            "id": "chatcmpl-fixture",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": "fixture-model",
            "choices": [{"index": 0, "message": response, "finish_reason": finish}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
        }
        self.json_response(payload)

    def json_response(self, payload):
        body = json.dumps(payload, separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        print(fmt % args, flush=True)


ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
