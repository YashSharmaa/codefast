#!/usr/bin/env python3
"""Compiler wrapper that records every invocation, then runs the real compiler.

Used as `make CC=/abs/path/cc_wrapper.py` to obtain a compilation database
without bear. Each invocation appends one JSON line
{"directory": cwd, "arguments": [real_cc, ...]} to $ORACLE_CC_LOG.
"""
import json
import os
import sys

real = os.environ.get("ORACLE_REAL_CC", "/usr/bin/clang")
log = os.environ.get("ORACLE_CC_LOG")
args = [real] + sys.argv[1:]
if log:
    line = json.dumps({"directory": os.getcwd(), "arguments": args}) + "\n"
    fd = os.open(log, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
    try:
        os.write(fd, line.encode())
    finally:
        os.close(fd)
os.execv(real, args)
