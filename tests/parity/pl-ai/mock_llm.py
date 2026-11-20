"""Local LLM/embedding mock for pl-ai (127.0.0.1 only, no third-party service).

Speaks the wire formats that llama_index 0.14 (Ollama, OpenAILike) and the upstream
embedding classes use:

* Ollama  ``POST /api/chat`` (``stream`` false → JSON, true → NDJSON), ``POST /api/embed``
* OpenAI  ``POST /v1/chat/completions`` (``stream`` false → JSON with tool call or text,
  true → SSE ``data: {...}``), ``POST /v1/embeddings`` (``encoding_format`` float/base64)

Responses are computed **deterministically** from the request text, so that Django and PaperRust
get the same response for the same request. Every request is logged (path + body) as a JSON line
in ``MOCK_LLM_LOG`` (if set) – the parity test compares the requests.

Invocation as a process:  python -I mock_llm.py <port> [<logfile>]
As a module: ``start(port=0) -> (server, port)``.
"""

import base64
import hashlib
import json
import re
import struct
import sys
import threading
from http.server import BaseHTTPRequestHandler
from http.server import ThreadingHTTPServer

DIM = 8
LOG_LOCK = threading.Lock()
CAPTURED: list = []
LOG_PATH = None
# Control via header/content: "MOCK_ERROR_500" in the prompt → 500, "MOCK_TIMEOUT" → slow
WORD = re.compile(r"\w+", re.UNICODE)


def embed(text: str) -> list[float]:
    v = [0.0] * DIM
    for w in WORD.findall(text.lower()):
        h = hashlib.sha256(w.encode("utf-8")).digest()
        v[h[0] % DIM] += 1.0 + (h[1] % 4) / 4.0
    if not any(v):
        v[0] = 1.0
    return v


def _prompt_of(body: dict) -> str:
    msgs = body.get("messages") or []
    return "\n".join(str(m.get("content") or "") for m in msgs)


def classification(prompt: str) -> dict:
    """Deterministic classification: title from the hash, names from candidates in the prompt."""
    h = hashlib.sha256(prompt.encode("utf-8")).hexdigest()
    out = {"title": f"Mock {h[:8]}", "tags": [], "matched_tags": [], "tag_ids": [],
           "correspondents": [], "matched_correspondents": [], "correspondent_ids": [],
           "document_types": [], "matched_document_types": [], "document_type_ids": [],
           "storage_paths": [], "matched_storage_paths": [], "storage_path_ids": [],
           "dates": ["2024-05-31"]}
    m = re.search(r"(\{\"(?:tags|document_types|correspondents|storage_paths)\".*\})\s*$", prompt, re.M)
    cands = {}
    if m:
        try:
            cands = json.loads(m.group(1))
        except ValueError:
            cands = {}
    for key, single in (("tags", "tag"), ("correspondents", "correspondent"),
                        ("document_types", "document_type"), ("storage_paths", "storage_path")):
        cs = cands.get(key) or []
        if cs:
            c = cs[int(h[2], 16) % len(cs)]
            out[key].append(c["name"])
            out[f"matched_{key}"].append(c["name"])
            out[f"{single}_ids"].append(c["id"])
    out["tags"].append("Mock Neu " + h[3])
    out["correspondents"].append("Mockfirma " + h[4].upper())
    if "Translate" in prompt or "language" in prompt.lower() and "suggestions" in prompt.lower():
        out["title"] = "Übersetzt " + h[:6]
    return out


def answer_text(prompt: str) -> str:
    h = hashlib.sha256(prompt.encode("utf-8")).hexdigest()
    return f"Antwort {h[:6]}: Die Dokumente nennen {len(prompt)} Zeichen. Ende."


def _chunks(text: str) -> list[str]:
    return re.findall(r"\S+\s*", text)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):  # noqa: D401
        pass

    def _send(self, code: int, data: bytes, ctype: str = "application/json"):
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _stream(self, ctype: str, parts: list[bytes]):
        self.send_response(200)
        self.send_header("content-type", ctype)
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        for p in parts:
            self.wfile.write(b"%x\r\n%s\r\n" % (len(p), p))
        self.wfile.write(b"0\r\n\r\n")

    def do_POST(self):  # noqa: N802, C901
        n = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(n) or b"{}"
        body = json.loads(raw)
        entry = {"path": self.path, "auth": self.headers.get("authorization"), "body": body}
        with LOG_LOCK:
            CAPTURED.append(entry)
            if LOG_PATH:
                with open(LOG_PATH, "a", encoding="utf-8") as f:
                    f.write(json.dumps(entry, ensure_ascii=False, sort_keys=True) + "\n")
        prompt = _prompt_of(body)
        if "MOCK_ERROR_500" in prompt:
            return self._send(500, b'{"error": "mock failure"}')
        if self.path.startswith("/api/embed"):
            inp = body.get("input")
            texts = inp if isinstance(inp, list) else [inp]
            return self._send(200, json.dumps({"model": body.get("model"),
                                               "embeddings": [embed(t) for t in texts]}).encode())
        if self.path.endswith("/embeddings"):
            inp = body.get("input")
            texts = inp if isinstance(inp, list) else [inp]
            data = []
            for i, t in enumerate(texts):
                e = embed(t)
                if body.get("encoding_format") == "base64":
                    e = base64.b64encode(struct.pack(f"<{len(e)}f", *e)).decode()
                data.append({"object": "embedding", "index": i, "embedding": e})
            return self._send(200, json.dumps({"object": "list", "data": data, "model": body.get("model"),
                                               "usage": {"prompt_tokens": 1, "total_tokens": 1}}).encode())
        structured = bool(body.get("format")) or bool(body.get("tools"))
        if self.path.startswith("/api/chat"):
            if structured:
                content = json.dumps(classification(prompt), ensure_ascii=False)
            else:
                content = answer_text(prompt)
            if body.get("stream"):
                lines = [json.dumps({"model": body.get("model"), "created_at": "2024-01-01T00:00:00Z",
                                     "message": {"role": "assistant", "content": c}, "done": False}).encode() + b"\n"
                         for c in _chunks(content)]
                lines.append(json.dumps({"model": body.get("model"), "created_at": "2024-01-01T00:00:00Z",
                                         "message": {"role": "assistant", "content": ""}, "done": True,
                                         "done_reason": "stop"}).encode() + b"\n")
                return self._stream("application/x-ndjson", lines)
            resp = {"model": body.get("model"), "created_at": "2024-01-01T00:00:00Z",
                    "message": {"role": "assistant", "content": content}, "done": True, "done_reason": "stop"}
            return self._send(200, json.dumps(resp, ensure_ascii=False).encode())
        # OpenAI-compatible
        if structured:
            msg = {"role": "assistant", "content": None, "tool_calls": [{
                "id": "c1", "type": "function", "function": {
                    "name": "DocumentClassifierSchema",
                    "arguments": json.dumps(classification(prompt), ensure_ascii=False)}}]}
            resp = {"id": "x", "object": "chat.completion", "created": 0, "model": body.get("model"),
                    "choices": [{"index": 0, "finish_reason": "tool_calls", "message": msg}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}
            return self._send(200, json.dumps(resp, ensure_ascii=False).encode())
        text = answer_text(prompt)
        if body.get("stream"):
            parts = []
            for c in _chunks(text):
                d = {"id": "x", "object": "chat.completion.chunk", "created": 0, "model": body.get("model"),
                     "choices": [{"index": 0, "delta": {"content": c}, "finish_reason": None}]}
                parts.append(b"data: " + json.dumps(d, ensure_ascii=False).encode() + b"\n\n")
            d = {"id": "x", "object": "chat.completion.chunk", "created": 0, "model": body.get("model"),
                 "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}
            parts.append(b"data: " + json.dumps(d).encode() + b"\n\n")
            parts.append(b"data: [DONE]\n\n")
            return self._stream("text/event-stream", parts)
        resp = {"id": "x", "object": "chat.completion", "created": 0, "model": body.get("model"),
                "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": text}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}
        return self._send(200, json.dumps(resp, ensure_ascii=False).encode())


def start(port: int = 0):
    srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]


if __name__ == "__main__":
    LOG_PATH = sys.argv[2] if len(sys.argv) > 2 else None
    srv = ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler)
    print("mock_llm on", srv.server_address[1], flush=True)
    srv.serve_forever()
