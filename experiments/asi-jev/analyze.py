#!/usr/bin/env python3
"""Summarize retained trials; run in CI, without provider access."""
import argparse
import json
from collections import defaultdict
from pathlib import Path

from benchmark import DIMENSIONS, load_events, phase_transition_metrics, summarize_metrics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, default=Path("experiments/asi-jev/results"))
    parser.add_argument("--events", type=Path, default=Path("experiments/asi-jev/data/events.jsonl"))
    args = parser.parse_args()
    corpus = {row["id"]: row for row in (
        json.loads(line) for line in args.events.read_text().splitlines() if line.strip()
    )}
    events, reference_sha256 = load_events(args.events)
    complete_ids = {event_id for event_id, row in corpus.items()
                    if row["origin"] == "synthetic_adversarial" and row["context"]["session_scope"] == "complete_session"}
    summaries = []
    cases = []
    trials = {}
    for path in sorted(args.results.rglob("*.summary.json")):
        summary = json.loads(path.read_text())
        stem = path.name.removesuffix(".summary.json")
        predictions = [json.loads(line) for line in path.with_name(stem + ".predictions.jsonl").read_text().splitlines() if line.strip()]
        provenance = json.loads(path.with_name(stem + ".provenance.json").read_text())
        rescored = []
        for prediction in predictions:
            reference = corpus[prediction["id"]]
            if prediction["native_sha256"] != reference["native"]["raw_sha256"]:
                raise SystemExit(f"Native input changed for {prediction['id']}; saved predictions cannot be rescored")
            rescored.append({**prediction, "gold": reference["gold"][prediction["gold_view"]],
                             "gold_reference_views": reference["gold"]})
        predictions = rescored
        summary = {**summary, "metrics": summarize_metrics(predictions),
                   "phase_transition_boundary": phase_transition_metrics(predictions, events)}
        if summary["strategy"] == "rules":
            summary["cost"] = {**summary["cost"], "estimate_usd": 0.0, "basis": "no_provider_requests"}
        # Repeated calls quantify variability, not additional independent events.
        stability = defaultdict(set)
        offline_reference = []
        for prediction in predictions:
            event = corpus[prediction["id"]]
            for dimension in DIMENSIONS:
                value = prediction["predictions"].get(dimension)
                stability[(prediction["id"], dimension)].add(json.dumps(value))
                if value != prediction["gold"][dimension]:
                    cases.append({"trial": stem, "id": prediction["id"], "repetition": prediction["repetition"],
                                  "origin": event["origin"], "dimension": dimension,
                                  "expected": prediction["gold"][dimension], "predicted": value,
                                  "validated_answer": prediction.get("validated_answers", {}).get(dimension),
                                  "annotation_note": event.get("annotation_note"),
                                  "error": prediction.get("errors", {}).get(dimension)})
            offline_reference.append({**prediction, "gold": event["gold"]["offline"]})
        key = (summary["strategy"], summary["mode"], summary["context"])
        if key in trials:
            raise SystemExit(f"Duplicate condition {key}: select one run per condition before aggregation")
        trials[key] = predictions
        summaries.append({"trial": stem, "original_summary_path": str(path), "provenance": provenance,
                          "scoring_reference_corpus_sha256": reference_sha256,
                          "summary": summary, "metrics_against_complete_session_gold": summarize_metrics(offline_reference),
                          "complete_authored_sessions": {"event_count": len(complete_ids),
                              "metrics": summarize_metrics([row for row in predictions if row["id"] in complete_ids]),
                              "metrics_against_offline_reference": summarize_metrics([
                                  row for row in offline_reference if row["id"] in complete_ids])},
                          "stability": {"event_dimension_pairs": len(stability),
                                        "pairs_with_different_repeated_labels": sum(len(values) > 1 for values in stability.values())}})
    paired = []
    for (strategy, mode, context), rows in trials.items():
        if context != "prefix" or (strategy, mode, "offline") not in trials:
            continue
        complete = {(row["id"], row["repetition"]): row for row in trials[(strategy, mode, "offline")]}
        for scope, selected in (("all", rows), ("complete_authored_sessions", [row for row in rows if row["id"] in complete_ids])):
            counts = {dimension: {"paired": 0, "label_changes": 0, "offline_correct_only": 0, "streaming_correct_only": 0} for dimension in DIMENSIONS}
            for row in selected:
                other = complete.get((row["id"], row["repetition"]))
                if other is None:
                    continue
                for dimension in DIMENSIONS:
                    left, right = row["predictions"].get(dimension), other["predictions"].get(dimension)
                    expected = corpus[row["id"]]["gold"]["offline"][dimension]
                    counts[dimension]["paired"] += 1
                    counts[dimension]["label_changes"] += left != right
                    counts[dimension]["offline_correct_only"] += right == expected and left != expected
                    counts[dimension]["streaming_correct_only"] += left == expected and right != expected
            paired.append({"strategy": strategy, "mode": mode, "scope": scope, "dimensions": counts})
    output = {"conditions": summaries, "streaming_vs_offline": paired,
              "scoring_reference_corpus_sha256": reference_sha256,
              "note": "Weak agent annotations; repetitions are not independent corpus samples. Scores are recomputed against the current corrected reference, after verifying each saved native input hash. Original measured summaries and predictions remain unchanged. Offline-reference scores use the same reference for comparability; fixture excerpts are not complete native streams."}
    args.results.mkdir(parents=True, exist_ok=True)
    (args.results / "comparison.json").write_text(json.dumps(output, indent=2) + "\n")
    (args.results / "failure-cases.jsonl").write_text("".join(json.dumps(row) + "\n" for row in cases))
    lines = ["# Measured results", "", output["note"], "",
             "| Strategy | Schedule | View | Activity accuracy | Failure accuracy | Retry accuracy | Anomaly accuracy | Phase accuracy | HTTP P50/P95 ms | Events/s | Requests | Est. USD |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for condition in summaries:
        summary = condition["summary"]
        metric = summary["metrics"]["overall"]
        accuracy = [f'{metric[dimension]["accuracy"]:.3f}' for dimension in DIMENSIONS]
        rtt = summary["latency"]["provider_http_rtt"]
        latency = "n/a" if rtt["p50_ms"] is None else f'{rtt["p50_ms"]:.1f}/{rtt["p95_ms"]:.1f}'
        cost = summary["cost"]["estimate_usd"]
        lines.append("| " + " | ".join([summary["strategy"], summary["mode"], summary["context"], *accuracy, latency,
                                        f'{summary["throughput"]["events_per_second_end_to_end_sequential"]:.2f}',
                                        str(summary["request_count"]), "unknown" if cost is None else f"{cost:.6f}"]) + " |")
    lines.extend(["", "See comparison.json for precision, recall, macro F1, confusion matrices, strata, phase transition metrics, repeated-label stability, and paired offline comparisons. See failure-cases.jsonl for every disagreement with the weak reference.", ""])
    (args.results / "RESULTS.md").write_text("\n".join(lines))
    print(json.dumps({"conditions": len(summaries), "failure_cases": len(cases), "output": str(args.results)}))


if __name__ == "__main__":
    main()
