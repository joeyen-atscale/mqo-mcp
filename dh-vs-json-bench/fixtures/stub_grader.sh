#!/usr/bin/env sh
# Stub grader for tests — reads the input JSON file and compares
# "reported" vs "correct" using an exact string match.
# Emits a GraderVerdict JSON on stdout.
#
# Usage: stub_grader.sh <input_json_file>
# Input JSON shape: { "reported": <value>, "correct": <value> }
# Output JSON shape: { "correct": bool, "error_class": str, "reason": str }
#
# This stub uses `python3` for JSON comparison (always available on macOS/Linux).
# If the values match exactly, it reports correct=true, error_class="correct".
# If they differ, it reports correct=false, error_class="arithmetic" (stub default).
#
# The real grader would perform semantic / numeric-tolerance comparison and
# classify errors as arithmetic / transcription / wrong_subset / correct.

input_file="$1"

python3 - "$input_file" <<'PYEOF'
import json, sys

data = json.load(open(sys.argv[1]))
reported = data.get("reported")
correct   = data.get("correct")

if reported == correct:
    print(json.dumps({"correct": True,  "error_class": "correct",    "reason": "stub grader: exact match"}))
else:
    print(json.dumps({"correct": False, "error_class": "arithmetic", "reason": f"stub grader: {reported!r} != {correct!r}"}))
PYEOF
