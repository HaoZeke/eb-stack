#!/usr/bin/env python3
"""Record the peak RSS of one compiler invocation, and summarise a log.

Launcher, for CMake and for a CC wrapper. The arguments are the real
compiler command. CMake runs
``CMAKE_C_COMPILER_LAUNCHER=<this script> <compiler> <args>``. A CC
wrapper is ``CC="<this script> gcc"``, which has the same shape.

Each invocation appends one JSON line to ``$EB_STACK_COMPILE_RSS_LOG``
(default ``compile-rss.jsonl`` in the working directory). The line holds
``argv``, the compiler's exit status, and
``getrusage(RUSAGE_CHILDREN).ru_maxrss``. On Linux that field is
kilobytes, and the log calls it ``ru_maxrss_kb``. Writes take an
exclusive lock so a parallel build does not tear a line.

Summary. ``compile_rss.py --summarize LOG`` reads that log, picks the
compile with the largest ``ru_maxrss_kb``, and suggests ``mib_per_job``:
the worst peak rounded up to a whole MiB, then grown by the headroom
(default one quarter, ``--headroom 0.25``) and rounded up again. The
suggestion is arithmetic on the log. It is not a measurement of a named
package, and it does not write ``data/job-memory.toml``.

The EESSI build flags to compile under, when measuring a package, are
``-O2 -ftree-vectorize -march=<target> -fno-math-errno``. Put them in
``CFLAGS`` and ``CXXFLAGS`` (and pass the same ``-march`` the bot builds
that target with) before the build that this launcher wraps. libxc's
peak has not been measured; do not invent one.
"""

from __future__ import annotations

import argparse
import fcntl
import fractions
import json
import os
import resource
import shlex
import subprocess
import sys

LOG_ENV = "EB_STACK_COMPILE_RSS_LOG"
DEFAULT_LOG = "compile-rss.jsonl"
DEFAULT_HEADROOM = "0.25"


def main(argv: list[str]) -> int:
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__.strip())
        print()
        print(f"  {os.path.basename(sys.argv[0])} COMPILER [ARG ...]")
        print(f"  {os.path.basename(sys.argv[0])} --summarize LOG [--headroom {DEFAULT_HEADROOM}]")
        return 0 if argv and argv[0] in ("-h", "--help") else 2
    if argv[0] == "--summarize":
        return summarize(argv[1:])
    return launch(argv)


def launch(cmd: list[str]) -> int:
    """Run `cmd` and append the children's peak RSS to the log."""
    log_path = os.environ.get(LOG_ENV, DEFAULT_LOG)
    try:
        completed = subprocess.run(cmd)
    except FileNotFoundError:
        print(f"compile_rss: cannot run {cmd[0]}", file=sys.stderr)
        return 127
    except OSError as error:
        print(f"compile_rss: cannot run {cmd[0]}: {error}", file=sys.stderr)
        return 127
    code = 1 if completed.returncode is None else completed.returncode
    record = compiler_record(cmd, code)
    if record is not None:
        append_record(log_path, record)
    if code < 0:
        signal_number = -code
        os.kill(os.getpid(), signal_number)
        return 128 + signal_number
    return code


def compiler_record(cmd: list[str], code: int) -> dict | None:
    """One log object, or None when ru_maxrss is not in kilobytes."""
    if not sys.platform.startswith("linux"):
        print(
            "compile_rss: ru_maxrss is recorded only on Linux, where it is kilobytes",
            file=sys.stderr,
        )
        return None
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    return {"argv": cmd, "exit": code, "ru_maxrss_kb": int(usage.ru_maxrss)}


def append_record(log_path: str, record: dict) -> None:
    """Append one JSON line. The lock keeps parallel compiles from tearing it."""
    line = json.dumps(record, separators=(",", ":")) + "\n"
    with open(log_path, "a", encoding="utf-8") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        handle.write(line)
        handle.flush()


def summarize(argv: list[str]) -> int:
    """Pick the worst compile in a log and suggest mib_per_job."""
    parser = argparse.ArgumentParser(prog="compile_rss.py --summarize")
    parser.add_argument("log", help="JSONL log written by the launcher")
    parser.add_argument(
        "--headroom",
        default=DEFAULT_HEADROOM,
        help="fraction added to the worst MiB, default %(default)s (one quarter)",
    )
    args = parser.parse_args(argv)
    try:
        headroom = fractions.Fraction(args.headroom)
    except ValueError:
        print(f"compile_rss: --headroom {args.headroom} is not a number", file=sys.stderr)
        return 2
    if headroom < 0:
        print("compile_rss: --headroom must be zero or positive", file=sys.stderr)
        return 2
    try:
        records = read_log(args.log)
    except OSError as error:
        print(f"compile_rss: {error}", file=sys.stderr)
        return 1
    except ValueError as error:
        print(f"compile_rss: {error}", file=sys.stderr)
        return 1
    if not records:
        print(f"compile_rss: {args.log} has no compile records", file=sys.stderr)
        return 1
    worst = max(records, key=lambda record: record["ru_maxrss_kb"])
    worst_kib = worst["ru_maxrss_kb"]
    worst_mib, suggested = suggest_mib(worst_kib, headroom)
    command = shlex.join(worst["argv"])
    print(f"compiles: {len(records)}")
    print(f"worst_ru_maxrss_kb: {worst_kib}")
    print(f"worst_command: {command}")
    print(f"worst_exit: {worst['exit']}")
    print(f"worst_mib: {worst_mib}")
    print(f"headroom: {headroom.numerator}/{headroom.denominator}")
    if worst_kib == 0:
        print("suggested_mib_per_job: none")
        print("note: the worst ru_maxrss is 0 KiB, so there is no mib_per_job to suggest")
    else:
        print(f"suggested_mib_per_job: {suggested}")
        print(
            "note: suggested_mib_per_job is the worst peak rounded up to MiB, "
            "plus headroom, rounded up; it is not a measurement of a named package"
        )
    return 0


def read_log(path: str) -> list[dict]:
    """Every complete record. A record without ru_maxrss_kb is an error."""
    records = []
    with open(path, encoding="utf-8") as handle:
        for number, line in enumerate(handle, start=1):
            if not line.strip():
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError as error:
                raise ValueError(f"{path}:{number}: {error}") from error
            if not isinstance(record, dict) or "ru_maxrss_kb" not in record:
                raise ValueError(f"{path}:{number}: record has no ru_maxrss_kb")
            try:
                kib = int(record["ru_maxrss_kb"])
            except (TypeError, ValueError) as error:
                raise ValueError(f"{path}:{number}: ru_maxrss_kb is not an integer") from error
            argv = record.get("argv")
            if not isinstance(argv, list) or not all(isinstance(part, str) for part in argv):
                raise ValueError(f"{path}:{number}: argv is not a list of strings")
            exit_code = record.get("exit", 0)
            if not isinstance(exit_code, int):
                raise ValueError(f"{path}:{number}: exit is not an integer")
            records.append({"argv": argv, "exit": exit_code, "ru_maxrss_kb": kib})
    return records


def suggest_mib(worst_kib: int, headroom: fractions.Fraction) -> tuple[int, int]:
    """Worst MiB, rounded up, and that value grown by `headroom`, rounded up.

    ``worst_mib`` is ``ceil(ru_maxrss_kb / 1024)``. The suggestion is
    ``ceil(worst_mib * (1 + headroom))``. Both are integers. A zero peak
    stays zero; the summary then suggests nothing.
    """
    if worst_kib < 0:
        raise ValueError("ru_maxrss_kb is negative")
    worst_mib = (worst_kib + 1023) // 1024
    numerator = worst_mib * (headroom.denominator + headroom.numerator)
    den = headroom.denominator
    suggested = (numerator + den - 1) // den
    return worst_mib, suggested


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
