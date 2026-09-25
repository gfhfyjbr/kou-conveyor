"""A deterministic Responses API implementation for the container smoke test."""

import json
import struct
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

RUNNING = (
    "Tool call is still running. Its result arrives in a later turn: "
    "continue with independent work, or end your turn to wait for it."
)


class Model(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with Path("/app/model-requests.jsonl").open("a") as log:
            log.write(json.dumps(request) + "\n")
        outputs = {
            item["call_id"]: item["output"]
            for item in request["input"]
            if item.get("type") == "function_call_output"
            and item["output"] != [{"type": "input_text", "text": RUNNING}]
        }
        calls = {
            item["call_id"]
            for item in request["input"]
            if item.get("type") == "function_call"
        }
        if "plain" not in calls:
            output = self.tool(
                "plain",
                "Bash",
                {
                    "command": "printf test; printf done > /app/answer",
                    "max_output_length": 40,
                },
            )
        elif "plain" not in outputs:
            output = self.message("Waiting for the first command.")
        elif "bounded" not in calls:
            output = self.tool(
                "bounded",
                "Bash",
                {
                    "command": 'python -c \'print("α" * 100 + "tail", end="")\'',
                    "max_output_length": 12,
                },
            )
        elif "bounded" not in outputs:
            output = self.message("Waiting for the second command.")
        elif "image" not in calls:
            output = self.tool("image", "ViewImage", {"path": "image.bmp"})
        elif "image" not in outputs:
            output = self.message("Waiting for the image.")
        else:
            output = self.message("done")
        response = {
            "id": "resp-" + str(len(request["input"])),
            "object": "response",
            "status": "completed",
            "output": [output],
            "usage": {
                "input_tokens": 10,
                "input_tokens_details": {"cached_tokens": 4},
                "output_tokens": 3,
                "output_tokens_details": {"reasoning_tokens": 1},
                "total_tokens": 13,
            },
        }
        if request.get("stream"):
            event = {"type": "response.completed", "response": response}
            body = f"event: response.completed\ndata: {json.dumps(event)}\n\n".encode()
            content_type = "text/event-stream"
        else:
            body = json.dumps(response).encode()
            content_type = "application/json"
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    @staticmethod
    def tool(call_id, name, arguments):
        return {
            "id": "fc-" + call_id,
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "status": "completed",
            "arguments": json.dumps(arguments),
        }

    @staticmethod
    def message(text):
        return {
            "id": "msg-" + text,
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": text}],
        }


if __name__ == "__main__":
    pixels = b"\x00\x80\xff" * 4000 * 2
    header = struct.pack("<2sIHHI", b"BM", 54 + len(pixels), 0, 0, 54)
    header += struct.pack(
        "<IIIHHIIIIII", 40, 4000, 2, 1, 24, 0, len(pixels), 0, 0, 0, 0
    )
    Path("/app/image.bmp").write_bytes(header + pixels)
    with HTTPServer(("0.0.0.0", 8765), Model) as server:
        server.serve_forever()
