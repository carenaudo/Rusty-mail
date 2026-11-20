"""Compare same-machine synthetic runs: python scripts/check-mail-performance.py baseline candidate."""
import json
import sys
from pathlib import Path

baseline_label, candidate_label = sys.argv[1:]
results = []
for count in (100000, 500000):
    def read(label):
        path = Path(f"reports/performance/conversations-{label}-{count}.json")
        data = json.loads(path.read_text(encoding="utf-8"))
        assert data["messages"] == count and data["encrypted"]
        return data
    baseline, candidate = read(baseline_label), read(candidate_label)
    assert baseline["build"] == candidate["build"], "Compare equal build profiles"
    assert baseline["fixture"] == candidate["fixture"], "Fixture shape changed"
    assert not any(k.endswith("_error") for k in candidate["timings"]), "Candidate query failed"
    assert "broad_search" in candidate["timings"], "Broad search must succeed"
    for action, before in baseline["timings"].items():
        if action.endswith("_error"):
            continue
        after = candidate["timings"][action]
        limit = before["p95_ms"] * 1.30 + 10
        assert after["p95_ms"] <= limit, f"{count} {action}: {after['p95_ms']:.2f} > {limit:.2f} ms"
        results.append({"messages": count, "action": action, "baseline_p95_ms": before["p95_ms"], "candidate_p95_ms": after["p95_ms"], "limit_ms": limit})
print(json.dumps({"passed": True, "baseline": baseline_label, "candidate": candidate_label, "checks": results}, indent=2))
