#!/usr/bin/env sh
# Stub grader for tests — always emits an "equivalent" verdict.
# Usage: stub_grader.sh <input_json_file>
# The real grader (slai-text-to-sql-accuracy-bench) reads the same format.
printf '{"equivalent":true,"reason":"stub grader: always equivalent"}\n'
