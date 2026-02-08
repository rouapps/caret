#!/usr/bin/env python3
"""
benchmark_vs_pandas.py — Caret vs Pandas vs HuggingFace Datasets
================================================================

Generates a 10 GB dummy JSONL file, then benchmarks:
  1. Time to first line
  2. Peak memory usage (RSS)
  3. Time to approximate deduplication

Outputs a Markdown table ready for the README.

Usage:
    python benchmark_vs_pandas.py [--size-gb 10] [--caret-bin ./target/release/caret]

Requirements:
    pip install pandas datasets psutil
"""

import argparse
import json
import os
import random
import resource
import shutil
import string
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# ─── Configuration ───────────────────────────────────────────────────────────

DEFAULT_SIZE_GB = 10
DUMMY_FILE = "benchmark_data.jsonl"
NUM_DEDUP_LINES = 50_000  # Smaller slice for dedup timing (full 10GB would OOM pandas)

PROMPTS = [
    "Explain quantum computing in simple terms.",
    "Write a Python function to sort a list.",
    "What is the meaning of life?",
    "How does photosynthesis work?",
    "Describe the theory of relativity.",
    "What are the benefits of exercise?",
    "Explain machine learning to a five-year-old.",
    "Write a haiku about programming.",
    "What is the Fibonacci sequence?",
    "How do neural networks learn?",
]

RESPONSES = [
    "Quantum computing uses qubits that can exist in superposition, "
    "allowing parallel computation of multiple states simultaneously.",
    "def sort_list(lst): return sorted(lst)",
    "The meaning of life is a philosophical question with many answers.",
    "Photosynthesis converts CO2 and water into glucose using sunlight.",
    "Relativity shows that space and time are interconnected.",
    "Exercise improves cardiovascular health, mood, and longevity.",
    "Machine learning is when computers learn patterns from examples.",
    "Bits and bytes flow / Logic gates open and close / Software comes alive",
    "The Fibonacci sequence: each number is the sum of the two before it.",
    "Neural networks adjust weights through backpropagation to minimize loss.",
]


def generate_dummy_jsonl(path: str, target_size_gb: float) -> int:
    """Generate a dummy JSONL file of approximately `target_size_gb` gigabytes.

    Returns the number of lines written.
    """
    target_bytes = int(target_size_gb * 1024 * 1024 * 1024)
    written = 0
    line_count = 0

    print(f"Generating {target_size_gb} GB JSONL file: {path}")
    start = time.time()

    with open(path, "w", buffering=1 << 20) as f:
        while written < target_bytes:
            # ~20% duplicate lines for dedup benchmarking
            if random.random() < 0.2 and line_count > 0:
                prompt = random.choice(PROMPTS)
                response = random.choice(RESPONSES)
            else:
                prompt = random.choice(PROMPTS) + " " + "".join(
                    random.choices(string.ascii_lowercase, k=random.randint(20, 200))
                )
                response = random.choice(RESPONSES) + " " + "".join(
                    random.choices(string.ascii_lowercase, k=random.randint(50, 500))
                )

            record = json.dumps(
                {
                    "prompt": prompt,
                    "response": response,
                    "metadata": {
                        "source": random.choice(["web", "book", "code", "wiki"]),
                        "quality": round(random.uniform(0.5, 1.0), 2),
                        "tokens": random.randint(50, 2000),
                    },
                },
                ensure_ascii=False,
            )

            line = record + "\n"
            f.write(line)
            written += len(line.encode("utf-8"))
            line_count += 1

            if line_count % 500_000 == 0:
                elapsed = time.time() - start
                pct = (written / target_bytes) * 100
                rate = written / (1024 * 1024 * elapsed) if elapsed > 0 else 0
                print(
                    f"  {pct:5.1f}% | {line_count:>10,} lines | "
                    f"{written / (1024**3):.2f} GB | {rate:.0f} MB/s",
                    flush=True,
                )

    elapsed = time.time() - start
    size_gb = written / (1024**3)
    print(f"Done: {line_count:,} lines, {size_gb:.2f} GB in {elapsed:.1f}s")
    return line_count


# ─── Benchmark Helpers ───────────────────────────────────────────────────────


def get_rss_mb():
    """Get current process RSS in MB (Unix only)."""
    try:
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
        # maxrss is in KB on Linux, bytes on macOS
        if sys.platform == "darwin":
            return usage.ru_maxrss / (1024 * 1024)
        else:
            return usage.ru_maxrss / 1024
    except Exception:
        return 0.0


def measure_rss_subprocess(cmd: list[str], timeout: int = 600) -> tuple[float, float]:
    """Run a command and measure wall-clock time + peak RSS.

    Returns (elapsed_seconds, peak_rss_mb).
    Uses /usr/bin/time on macOS/Linux for accurate RSS measurement.
    """
    time_cmd = "/usr/bin/time"
    if not os.path.exists(time_cmd):
        # Fallback: just measure wall time
        start = time.time()
        result = subprocess.run(cmd, capture_output=True, timeout=timeout)
        elapsed = time.time() - start
        return elapsed, 0.0

    if sys.platform == "darwin":
        full_cmd = [time_cmd, "-l"] + cmd
    else:
        full_cmd = [time_cmd, "-v"] + cmd

    start = time.time()
    result = subprocess.run(
        full_cmd, capture_output=True, text=True, timeout=timeout
    )
    elapsed = time.time() - start

    # Parse RSS from /usr/bin/time output (it goes to stderr)
    stderr = result.stderr
    rss_mb = 0.0

    if sys.platform == "darwin":
        # macOS: "  12345678  maximum resident set size" (in bytes)
        for line in stderr.splitlines():
            if "maximum resident set size" in line:
                try:
                    rss_mb = int(line.strip().split()[0]) / (1024 * 1024)
                except (ValueError, IndexError):
                    pass
    else:
        # Linux: "Maximum resident set size (kbytes): 12345"
        for line in stderr.splitlines():
            if "Maximum resident set size" in line:
                try:
                    rss_mb = int(line.strip().split()[-1]) / 1024
                except (ValueError, IndexError):
                    pass

    return elapsed, rss_mb


# ─── Benchmarks ──────────────────────────────────────────────────────────────


def bench_time_to_first_line(data_path: str, caret_bin: str) -> dict:
    """Measure time-to-first-line for each tool."""
    results = {}

    # --- Caret ---
    print("  [caret] time to first line...", flush=True)
    start = time.time()
    proc = subprocess.run(
        [caret_bin, data_path, "--dedup"],
        capture_output=True,
        text=True,
        timeout=300,
    )
    caret_time = time.time() - start
    # Caret prints "Loaded X lines" to stderr almost instantly
    # The real "first line" time is the time to get the first output
    # For a fairer comparison, measure just the load time from stderr
    for line in proc.stderr.splitlines():
        if "Loaded" in line:
            # Caret loaded the file (mmap = instant)
            break
    results["caret"] = caret_time

    # --- Pandas ---
    print("  [pandas] time to first line...", flush=True)
    pandas_script = f"""
import time, pandas as pd
start = time.time()
for chunk in pd.read_json("{data_path}", lines=True, chunksize=1):
    first = chunk.iloc[0]
    break
print(f"TTFL={{time.time() - start:.6f}}")
"""
    start = time.time()
    result = subprocess.run(
        [sys.executable, "-c", pandas_script],
        capture_output=True,
        text=True,
        timeout=300,
    )
    pandas_time = time.time() - start
    # Extract precise TTFL from output
    for line in result.stdout.splitlines():
        if line.startswith("TTFL="):
            pandas_time = float(line.split("=")[1])
    results["pandas"] = pandas_time

    # --- HuggingFace Datasets ---
    print("  [hf-datasets] time to first line...", flush=True)
    hf_script = f"""
import time
from datasets import load_dataset
start = time.time()
ds = load_dataset("json", data_files="{data_path}", split="train", streaming=True)
first = next(iter(ds))
print(f"TTFL={{time.time() - start:.6f}}")
"""
    start = time.time()
    result = subprocess.run(
        [sys.executable, "-c", hf_script],
        capture_output=True,
        text=True,
        timeout=300,
    )
    hf_time = time.time() - start
    for line in result.stdout.splitlines():
        if line.startswith("TTFL="):
            hf_time = float(line.split("=")[1])
    results["hf_datasets"] = hf_time

    return results


def bench_memory_usage(data_path: str, caret_bin: str) -> dict:
    """Measure peak RSS for loading the full dataset."""
    results = {}

    # --- Caret (loads via mmap — should be near-zero RSS) ---
    print("  [caret] memory usage...", flush=True)
    elapsed, rss = measure_rss_subprocess(
        [caret_bin, data_path, "--dedup"]
    )
    results["caret"] = rss

    # --- Pandas ---
    print("  [pandas] memory usage...", flush=True)
    pandas_script = f"""
import pandas as pd
df = pd.read_json("{data_path}", lines=True)
print(f"ROWS={{len(df)}}")
"""
    elapsed, rss = measure_rss_subprocess(
        [sys.executable, "-c", pandas_script]
    )
    results["pandas"] = rss

    # --- HuggingFace Datasets ---
    print("  [hf-datasets] memory usage...", flush=True)
    hf_script = f"""
from datasets import load_dataset
ds = load_dataset("json", data_files="{data_path}", split="train")
print(f"ROWS={{len(ds)}}")
"""
    elapsed, rss = measure_rss_subprocess(
        [sys.executable, "-c", hf_script]
    )
    results["hf_datasets"] = rss

    return results


def bench_dedup_time(data_path: str, caret_bin: str) -> dict:
    """Measure approximate dedup time."""
    results = {}

    # Create a smaller file for pandas/hf dedup (they'd OOM on 10GB)
    small_path = data_path + ".dedup_sample.jsonl"
    print(f"  Creating {NUM_DEDUP_LINES:,}-line dedup sample...", flush=True)

    with open(data_path) as fin, open(small_path, "w") as fout:
        for i, line in enumerate(fin):
            if i >= NUM_DEDUP_LINES:
                break
            fout.write(line)

    # --- Caret (operates on the full file via SIMD) ---
    print("  [caret] dedup scan (full file)...", flush=True)
    start = time.time()
    proc = subprocess.run(
        [caret_bin, data_path, "--dedup"],
        capture_output=True,
        text=True,
        timeout=600,
    )
    results["caret"] = time.time() - start

    # --- Pandas (on sample) ---
    print(f"  [pandas] dedup ({NUM_DEDUP_LINES:,} lines)...", flush=True)
    pandas_dedup = f"""
import time, pandas as pd
start = time.time()
df = pd.read_json("{small_path}", lines=True)
before = len(df)
df = df.drop_duplicates(subset=["prompt", "response"])
after = len(df)
elapsed = time.time() - start
print(f"DEDUP={{elapsed:.6f}} before={{before}} after={{after}}")
"""
    start = time.time()
    result = subprocess.run(
        [sys.executable, "-c", pandas_dedup],
        capture_output=True,
        text=True,
        timeout=600,
    )
    pandas_time = time.time() - start
    for line in result.stdout.splitlines():
        if line.startswith("DEDUP="):
            pandas_time = float(line.split("=")[1].split()[0])
    results["pandas"] = pandas_time

    # --- HuggingFace Datasets (on sample) ---
    print(f"  [hf-datasets] dedup ({NUM_DEDUP_LINES:,} lines)...", flush=True)
    hf_dedup = f"""
import time, hashlib
from datasets import load_dataset
start = time.time()
ds = load_dataset("json", data_files="{small_path}", split="train")
seen = set()
def dedup(example):
    h = hashlib.md5((example.get("prompt","") + example.get("response","")).encode()).hexdigest()
    if h in seen:
        return False
    seen.add(h)
    return True
ds_deduped = ds.filter(dedup)
elapsed = time.time() - start
print(f"DEDUP={{elapsed:.6f}} before={{len(ds)}} after={{len(ds_deduped)}}")
"""
    start = time.time()
    result = subprocess.run(
        [sys.executable, "-c", hf_dedup],
        capture_output=True,
        text=True,
        timeout=600,
    )
    hf_time = time.time() - start
    for line in result.stdout.splitlines():
        if line.startswith("DEDUP="):
            hf_time = float(line.split("=")[1].split()[0])
    results["hf_datasets"] = hf_time

    # Cleanup
    os.remove(small_path)

    return results


# ─── Output ──────────────────────────────────────────────────────────────────


def format_time(seconds: float) -> str:
    if seconds < 0.001:
        return f"{seconds * 1_000_000:.0f} us"
    if seconds < 1.0:
        return f"{seconds * 1000:.0f} ms"
    if seconds < 60:
        return f"{seconds:.1f} s"
    return f"{seconds / 60:.1f} min"


def format_memory(mb: float) -> str:
    if mb == 0:
        return "N/A"
    if mb < 1024:
        return f"{mb:.0f} MB"
    return f"{mb / 1024:.1f} GB"


def print_markdown_table(
    ttfl: dict, memory: dict, dedup: dict, data_size_gb: float, line_count: int
):
    """Print a Markdown table suitable for the README."""
    print()
    print("## Benchmark Results")
    print()
    print(f"**Dataset:** {data_size_gb:.1f} GB JSONL ({line_count:,} lines)")
    print()
    print("| Metric | Caret | Pandas | HF Datasets |")
    print("|--------|-------|--------|-------------|")
    print(
        f"| Time to First Line | **{format_time(ttfl['caret'])}** | "
        f"{format_time(ttfl['pandas'])} | {format_time(ttfl['hf_datasets'])} |"
    )
    print(
        f"| Peak Memory (RSS) | **{format_memory(memory['caret'])}** | "
        f"{format_memory(memory['pandas'])} | {format_memory(memory['hf_datasets'])} |"
    )
    print(
        f"| Dedup Time | **{format_time(dedup['caret'])}** | "
        f"{format_time(dedup['pandas'])}* | {format_time(dedup['hf_datasets'])}* |"
    )
    print()
    print(
        f"*\\*Pandas and HF Datasets dedup measured on {NUM_DEDUP_LINES:,}-line "
        f"sample (full {data_size_gb:.0f} GB would OOM). Caret runs on the full file.*"
    )
    print()
    print("**Environment:**")
    print(f"- OS: {sys.platform}")

    try:
        import platform
        print(f"- CPU: {platform.processor() or platform.machine()}")
    except Exception:
        pass

    try:
        import psutil
        ram_gb = psutil.virtual_memory().total / (1024**3)
        print(f"- RAM: {ram_gb:.0f} GB")
    except ImportError:
        pass

    print(f"- Python: {sys.version.split()[0]}")
    print()


# ─── Main ────────────────────────────────────────────────────────────────────


def main():
    parser = argparse.ArgumentParser(
        description="Benchmark Caret vs Pandas vs HuggingFace Datasets"
    )
    parser.add_argument(
        "--size-gb",
        type=float,
        default=DEFAULT_SIZE_GB,
        help=f"Size of generated JSONL file in GB (default: {DEFAULT_SIZE_GB})",
    )
    parser.add_argument(
        "--caret-bin",
        type=str,
        default="./target/release/caret",
        help="Path to the caret binary (default: ./target/release/caret)",
    )
    parser.add_argument(
        "--data-path",
        type=str,
        default=DUMMY_FILE,
        help=f"Path for generated data file (default: {DUMMY_FILE})",
    )
    parser.add_argument(
        "--skip-generate",
        action="store_true",
        help="Skip data generation (reuse existing file)",
    )
    args = parser.parse_args()

    # Verify caret binary exists
    caret_bin = args.caret_bin
    if not os.path.exists(caret_bin):
        # Try building it
        print(f"Caret binary not found at {caret_bin}, building...")
        subprocess.run(
            ["cargo", "build", "--release"],
            check=True,
        )
        if not os.path.exists(caret_bin):
            print(f"ERROR: Could not find caret binary at {caret_bin}")
            print("Build with: cargo build --release")
            sys.exit(1)

    # Verify Python dependencies
    for pkg in ["pandas", "datasets"]:
        try:
            __import__(pkg)
        except ImportError:
            print(f"ERROR: Missing Python package '{pkg}'")
            print(f"Install with: pip install {pkg}")
            sys.exit(1)

    # Generate test data
    data_path = args.data_path
    if args.skip_generate and os.path.exists(data_path):
        line_count = sum(1 for _ in open(data_path))
        size_gb = os.path.getsize(data_path) / (1024**3)
        print(f"Reusing existing file: {data_path} ({size_gb:.2f} GB, {line_count:,} lines)")
    else:
        line_count = generate_dummy_jsonl(data_path, args.size_gb)
        size_gb = os.path.getsize(data_path) / (1024**3)

    print()
    print("=" * 60)
    print("  BENCHMARK: Caret vs Pandas vs HuggingFace Datasets")
    print(f"  Data: {size_gb:.1f} GB JSONL ({line_count:,} lines)")
    print("=" * 60)

    # Run benchmarks
    print()
    print("[1/3] Time to First Line")
    ttfl = bench_time_to_first_line(data_path, caret_bin)

    print()
    print("[2/3] Peak Memory Usage (RSS)")
    memory = bench_memory_usage(data_path, caret_bin)

    print()
    print("[3/3] Deduplication Time")
    dedup = bench_dedup_time(data_path, caret_bin)

    # Output results
    print()
    print("=" * 60)
    print_markdown_table(ttfl, memory, dedup, size_gb, line_count)

    # Cleanup prompt
    if os.path.exists(data_path):
        answer = input(f"\nDelete generated file {data_path}? [y/N] ").strip().lower()
        if answer == "y":
            os.remove(data_path)
            print("Deleted.")


if __name__ == "__main__":
    main()
