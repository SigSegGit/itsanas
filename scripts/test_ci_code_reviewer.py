# -*- coding: utf-8 -*-
"""Tests for `ci_code_reviewer.py`, against a fake endpoint on localhost.

Why these and not others
------------------------

The reviewer's job is to fail loudly and to send only what it says it sends.
Each case below is a way it could quietly do otherwise, and each was checked
by breaking the code it guards (2026-09-24): without retries the two
transient cases fail; without splitting `AI_MODEL_NAME` the three fallback
cases fail.

No network, no key: the fake endpoint speaks just enough of the OpenAI
protocol. Run by `.github/workflows/ai-code-reviewer.yml` before the real
review, so a broken reviewer is named as such rather than mid-review.

    python scripts/test_ci_code_reviewer.py      # needs the `openai` package
"""

import contextlib
import http.server
import io
import json
import os
import sys
import threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ci_code_reviewer as c  # noqa: E402

c.RETRY_WAITS_SECONDS = (0, 0)

plan = {}   # model -> status codes in order, the last one repeating
calls = []


class Fake(http.server.BaseHTTPRequestHandler):
    def _send(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):  # the model listing printed when nothing answers
        self._send(200, {"object": "list", "data": [
            {"id": "models/usable", "object": "model", "created": 0, "owned_by": "x"}]})

    def do_POST(self):
        model = json.loads(self.rfile.read(int(self.headers["Content-Length"])))["model"]
        seq = plan.get(model, [404])
        code = seq[min(calls.count(model), len(seq) - 1)]
        calls.append(model)
        if code == 200:
            self._send(200, {"id": "x", "object": "chat.completion", "created": 0,
                             "model": model, "choices": [{
                                 "index": 0, "finish_reason": "stop",
                                 "message": {"role": "assistant", "content": "nothing found"}}]})
        else:
            self._send(code, {"error": {"code": code, "message": f"fake {code}"}})

    def log_message(self, *args):
        pass


def run_review(models, codes):
    plan.clear()
    plan.update(codes)
    calls.clear()
    os.environ["AI_MODEL_NAME"] = models
    err = io.StringIO()
    try:
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            used, _ = c.review("diff")
        return f"ok:{used}", len(calls)
    except SystemExit:
        return ("fail:listed" if "models/usable" in err.getvalue() else "fail"), len(calls)


def main():
    server = http.server.HTTPServer(("127.0.0.1", 0), Fake)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    os.environ.update(AI_API_KEY="fake",
                      AI_BASE_URL=f"http://127.0.0.1:{server.server_port}/v1")

    failures = []

    def expect(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got}, want {want}")

    # (name, AI_MODEL_NAME, per-model status codes, expected result, expected calls)
    cases = [
        ("an overload on one model recovers on retry", "a", {"a": [503, 503, 200]}, "ok:a", 3),
        ("a rate limit recovers on retry", "a", {"a": [429, 200]}, "ok:a", 2),
        ("an overloaded model hands over to the next", "a,b", {"a": [503], "b": [200]}, "ok:b", 4),
        ("a retired model hands over to the next", "x,b", {"x": [404], "b": [200]}, "ok:b", 2),
        ("nothing answering fails and names the usable models", "a", {"a": [503]},
         "fail:listed", 3),
        ("a bad key fails at once, without trying the next model", "a,b",
         {"a": [401], "b": [200]}, "fail", 1),
    ]
    for name, models, codes, want, want_calls in cases:
        got, n = run_review(models, codes)
        expect(name, (got, n), (want, want_calls))

    # Truncation keeps whole files, names the ones it dropped, and cuts a single
    # oversized file rather than sending nothing.
    diff = "".join(f"diff --git a/f{i} b/f{i}\n+{'x' * 100}\n" for i in range(5))
    kept, omitted = c.truncate(diff, 300)
    expect("truncation keeps whole files", kept.count("diff --git"), 2)
    expect("truncation names what it dropped", omitted, ["f2", "f3", "f4"])
    kept, omitted = c.truncate(diff, 50)
    expect("an oversized first file is cut, not dropped",
           (kept.startswith("diff --git a/f0"), omitted[0]), (True, "f0 (partially)"))
    expect("a diff under the cap is untouched", c.truncate(diff, 10**6), (diff, []))

    server.shutdown()
    if failures:
        print("ci_code_reviewer tests FAILED:")
        for f in failures:
            print(f"  {f}")
        sys.exit(1)
    print(f"ci_code_reviewer: {len(cases) + 4} checks passed")


if __name__ == "__main__":
    main()
