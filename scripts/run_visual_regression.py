#!/usr/bin/env python3
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CASES = ROOT / "fixtures" / "render" / "cases.json"

def run_json(command):
    completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(map(str, command))}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return json.loads(completed.stdout)

def render_args(binary, case, output):
    args = [str(binary), "render", "asset", str((CASES.parent / case["asset"]).resolve())]
    if "drawableIndex" in case:
        args += ["--drawable-index", str(case["drawableIndex"])]
    args += ["--output", str(output), "--json"]
    options = case["options"]
    for name in ("width", "height", "view", "projection"):
        if name in options:
            args += [f"--{name.replace('_', '-')}", str(options[name])]
    for flag in ("transparent", "grid", "wireframe", "bounds"):
        if options.get(flag):
            args.append(f"--{flag}")
    return args

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ragelab", default=str(ROOT / "target" / "debug" / ("ragelab.exe" if __import__("os").name == "nt" else "ragelab")))
    parser.add_argument("--tolerance", type=int, default=0)
    args = parser.parse_args()
    binary = Path(args.ragelab)
    if not binary.is_file():
        raise SystemExit(f"ragelab binary not found: {binary}")

    manifest = json.loads(CASES.read_text(encoding="utf-8"))
    reports = []
    with tempfile.TemporaryDirectory(prefix="ragelab-visual-") as temp:
        temp = Path(temp)
        for case in manifest["cases"]:
            left = temp / f"{case['id']}-a.png"
            right = temp / f"{case['id']}-b.png"
            a = run_json(render_args(binary, case, left))
            b = run_json(render_args(binary, case, right))
            summary = a["data"]["package"]["summary"]
            for key, expected in case["expectedPackage"].items():
                actual = summary[key]
                if actual != expected:
                    raise RuntimeError(f"{case['id']} expected {key}={expected}, got {actual}")
            comparison = run_json([
                str(binary), "render", "compare", str(left), str(right),
                "--tolerance", str(args.tolerance), "--json"
            ])
            report = comparison["data"]
            if not report["withinTolerance"]:
                raise RuntimeError(f"{case['id']} visual regression failed: {report}")
            reports.append({
                "id": case["id"],
                "imageSha256": a["data"]["screenshot"]["imageSha256"],
                "repeatSha256": b["data"]["screenshot"]["imageSha256"],
                "changedPixels": report["changedPixels"],
                "maxChannelDelta": report["maxChannelDelta"],
                "tolerance": report["tolerance"],
            })
    print(json.dumps({
        "schema": "ragelab.render.visual-regression-suite",
        "schemaVersion": 1,
        "cases": reports,
    }, separators=(",", ":")))

if __name__ == "__main__":
    main()
