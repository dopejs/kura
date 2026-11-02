"""OpenAI-compatible fake model for the tenant-browser end to end.

Round 1 (no tool result yet): asks for the browser-open tool with the URL
named in the user's message (the download tool instead when the message
says "download"). Round 2: answers "PAGE SAID: <first 300 chars
of the tool result>" so the run can check what the browser read. Logs every
call (key, tool names offered, whether a tool result was present).
"""
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = sys.argv[2]


def reply(handler, body, stream):
    if stream:
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.end_headers()
        msg = body["choices"][0]["message"]
        delta = {"role": "assistant"}
        if msg.get("tool_calls"):
            delta["tool_calls"] = [dict(tc, index=i) for i, tc in enumerate(msg["tool_calls"])]
        else:
            delta["content"] = msg["content"]
        chunks = [
            {"choices": [{"index": 0, "delta": delta}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": body["choices"][0]["finish_reason"]}],
             "usage": body["usage"]},
        ]
        for c in chunks:
            handler.wfile.write(f"data: {json.dumps(c)}\n\n".encode())
        handler.wfile.write(b"data: [DONE]\n\n")
        return
    out = json.dumps(body).encode()
    handler.send_response(200)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(out)))
    handler.end_headers()
    handler.wfile.write(out)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        messages = req.get("messages", [])
        tools = [t.get("function", {}).get("name", "") for t in req.get("tools", [])]
        tool_results = [m for m in messages if m.get("role") == "tool"]
        key = self.headers.get("Authorization", "").removeprefix("Bearer ")
        with open(LOG, "a") as f:
            f.write(json.dumps({"key": key, "tools": tools, "toolResults": len(tool_results)}) + "\n")
        usage = {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        user = next((m for m in reversed(messages) if m.get("role") == "user"), {})
        content = user.get("content")
        if isinstance(content, list):
            content = " ".join(part.get("text", "") for part in content if isinstance(part, dict))
        wanted = r"browser.?download" if "download" in (content or "").lower() else r"browser.?open"
        open_tool = next((t for t in tools if re.search(wanted, t)), None)
        url = re.search(r"https?://\S+", content or "")
        if not tool_results and open_tool and url:
            msg = {"role": "assistant", "content": None, "tool_calls": [{
                "id": "call_open", "type": "function",
                "function": {"name": open_tool, "arguments": json.dumps({"url": url.group(0)})}}]}
            body = {"choices": [{"index": 0, "message": msg, "finish_reason": "tool_calls"}], "usage": usage}
        else:
            seen = tool_results[-1].get("content", "") if tool_results else "(no tool result)"
            if isinstance(seen, list):
                seen = " ".join(part.get("text", "") for part in seen if isinstance(part, dict))
            text = "PAGE SAID: " + " ".join(str(seen).split())[:300]
            body = {"choices": [{"index": 0, "message": {"role": "assistant", "content": text},
                                 "finish_reason": "stop"}], "usage": usage}
        reply(self, body, bool(req.get("stream")))


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
