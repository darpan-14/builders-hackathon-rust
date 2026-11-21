"""Time the two production binaries inside isolated containers on the same machine."""
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def timed(work, data, name, threads):
    out = work / ("out-" + name)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(mode=0o777)
    out.chmod(0o777)
    # Keep directory ownership with the host even when the container UID differs.
    for table in ["user_daily", "campaign_daily"]:
        (out / table).mkdir()
        (out / table).chmod(0o777)
    binary = "/usr/local/bin/shopping-sessions" if name == "rust" else "/app/shopping-sessions"
    # Measure the child from start to exit inside the container, including both writes.
    shell = 'set -eu; start=$(date +%s%N); "$1"; end=$(date +%s%N); echo $((end-start))'
    result = run("docker", "run", "--rm", "--network=none", "--read-only",
                 "--user=1000:1000", "--cap-drop=ALL", "--security-opt=no-new-privileges",
                 "--tmpfs=/tmp:rw,size=64m,mode=1777", "--cpus", str(threads),
                 "--memory=12g", "--memory-swap=12g", "-e", f"NCPU={threads}",
                 "-e", "MEM_GB=12", "-v", f"{data}:/data:ro", "-v", f"{out}:/output",
                 "-v", f"{work / 'scratch'}:/scratch", "--entrypoint=sh", "shopping-" + name,
                 "-c", shell, "bench", binary, text=True, stdout=subprocess.PIPE)
    return int(result.stdout.strip()) / 1e9


def main():
    work = Path(sys.argv[1]).resolve()
    threads = min(4, len(os.sched_getaffinity(0)))
    (work / "scratch").mkdir(exist_ok=True)
    (work / "scratch").chmod(0o777)
    stats = []
    for seed, events, measured in [(17, 20_000_000, 3), (29, 20_000_000, 3), (39, 60_000_000, 1)]:
        data = work / f"seed-{seed}"
        run(work / "generate", "--seed", str(seed), "--events", str(events),
            "--clicks", str(events // 4), "--threads", str(threads), "--output", data)
        times = {"rust": [], "cpp": []}
        for i in range(measured + 1):
            for name in (["rust", "cpp"] if i % 2 == 0 else ["cpp", "rust"]):
                elapsed = timed(work, data, name, threads)
                print(f"seed={seed} round={i} {name} {elapsed:.4f}s", flush=True)
                if i:
                    times[name].append(elapsed)
        run(sys.executable, Path(__file__).with_name("compare.py"), work / "out-cpp", work / "out-rust")
        stats.append(f"| {seed} | {events:,} | {measured} | {statistics.median(times['rust']):.3f} | "
                     f"{statistics.median(times['cpp']):.3f} |")
        shutil.rmtree(data)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
        print(f"Fresh-process wall seconds on {threads} x64 vCPUs. The first run of each image "
              "warms the cache. Both tables compared exactly on each seed.\n\n"
              "| Seed | Events | Measured runs per image | Rust | C++ |\n"
              "|---:|---:|---:|---:|---:|\n" + "\n".join(stats), file=f)


if __name__ == "__main__":
    main()
