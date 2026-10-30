#!/usr/bin/env python3
"""Produce disclosure-safe aggregate metrics for the frozen real-session protocol.

The input files stay local and private. Reference JSONL rows have this shape::

    {"id":"private event key", "session_id":"private session key",
     "harness":"claude", "event_order":1, "complete_session":true,
     "eligible_event_count":12,
     "gold":{"activity":"progress", "failure_category":"none",
       "retry_loop":"none", "anomaly":false, "phase":"active"},
     "causal_gold":{"activity":"progress", ...}}

``gold`` is the adjudicated offline reference. ``causal_gold`` is optional and
is scored separately for prefix predictions. ``complete_session`` and
``eligible_event_count`` are required to include a session in alert-budget
statistics; every eligible event from that complete session must be present.
Prediction JSONL rows use benchmark.py's ``id``, ``repetition``, ``strategy``,
``mode``, ``context`` and ``predictions`` fields. Each condition/context must
have one row per event and repetition. Optional benchmark summary JSON files
provide per-run request latency percentiles, request-failure counts, and token
totals. The optional disclosure-safe ``--operations`` JSON comes from
``real_operations.py`` and supplies pooled latency percentiles and counts for
matching each condition's events, prediction rows, and requests. Independent annotation and user-review JSONL rows have ``id`` plus
``gold`` and optional ``causal_gold`` objects with the same taxonomy fields.

Acceptance additionally requires at least 300 unique events, 15 complete
sessions with at least five from each harness, blind independent annotations
covering every reference event for both offline and causal labels, and
``--adjudication-status frozen``, exactly 2,000 bootstrap draws with seed
20261030, and the frozen repetition sets (one Rules run and three remote runs
for every required context). Protocol acceptance also requires the operations
aggregate; exploratory scoring remains available without it. The status enum is
``pending`` or ``frozen``. User spot-check
review remains separately marked pending until its local review file is given.

Only aggregate counts, metric values, confidence bounds, fixed taxonomy names,
and fixed status values are written. Pick an output path outside ``private/``.
The output contains no input paths, event/session identifiers, hashes,
timestamps, examples, or per-event records.
"""

from __future__ import annotations

import argparse
import json
import math
import random
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Callable, Iterable


DIMENSIONS: dict[str, tuple[str, ...]] = {
    "activity": ("task_request", "progress", "tool_action", "wait", "failure", "recovery", "completion", "unknown"),
    "failure_category": ("none", "tool", "permission", "environment", "model", "transport", "malformed", "unknown"),
    "retry_loop": ("none", "retry", "loop"),
    "anomaly": ("false", "true"),
    "phase": ("start", "active", "blocked", "recovery", "ending", "unknown"),
    "failure_detection": ("no_failure", "failure"),
}
ABSTAIN = "<abstain>"
CONTEXTS = {"prefix", "offline"}
MODES = {"per-event", "microbatch", "event-triggered"}
PRIMARY_CONDITIONS = (
    "Jev per-event", "Jev microbatch", "Rules+triggeredJev",
    "Haiku per-event", "Rules+triggeredHaiku",
)
OPERATIONS_CONDITIONS = {
    ("Rules", "prefix"): "rules-prefix",
    ("Rules", "offline"): "rules-offline",
    ("Jev per-event", "prefix"): "jev-prefix",
    ("Jev per-event", "offline"): "jev-offline",
    ("Jev microbatch", "prefix"): "jev-microbatch-prefix",
    ("Rules+triggeredJev", "prefix"): "jev-triggered-prefix",
    ("Haiku per-event", "prefix"): "llm-prefix",
    ("Haiku per-event", "offline"): "llm-offline",
    ("Rules+triggeredHaiku", "prefix"): "llm-triggered-prefix",
}
EXPECTED_REPETITIONS = {
    ("Rules", "prefix"): (1,),
    ("Rules", "offline"): (1,),
    ("Jev per-event", "prefix"): (1, 2, 3),
    ("Jev per-event", "offline"): (1, 2, 3),
    ("Jev microbatch", "prefix"): (1, 2, 3),
    ("Rules+triggeredJev", "prefix"): (1, 2, 3),
    ("Haiku per-event", "prefix"): (1, 2, 3),
    ("Haiku per-event", "offline"): (1, 2, 3),
    ("Rules+triggeredHaiku", "prefix"): (1, 2, 3),
}
DEFAULT_SEED = 20261030
DEFAULT_DRAWS = 2000
CONFIDENCE = 0.95
MAX_INPUT_BYTES = 10_000_000


class MetricsError(ValueError):
    """An invalid private metrics input; messages never include input values."""


def condition_name(strategy: Any, mode: Any) -> str:
    """Map benchmark strategy/mode fields to the protocol's fixed names."""
    if not isinstance(strategy, str) or strategy not in {"rules", "jev", "llm"} or not isinstance(mode, str) or mode not in MODES:
        raise MetricsError("unsupported condition fields")
    if strategy == "llm" and mode == "microbatch":
        raise MetricsError("unsupported condition fields")
    if strategy == "rules":
        if mode != "per-event":
            raise MetricsError("unsupported condition fields")
        return "Rules"
    if strategy == "jev":
        return {
            "per-event": "Jev per-event",
            "microbatch": "Jev microbatch",
            "event-triggered": "Rules+triggeredJev",
        }[mode]
    return {
        "per-event": "Haiku per-event",
        "event-triggered": "Rules+triggeredHaiku",
    }[mode]


def _label(value: Any, dimension: str, *, allow_abstain: bool = False) -> str:
    if dimension == "anomaly" and isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str) and value in DIMENSIONS[dimension]:
        return value
    if allow_abstain and value is None:
        return ABSTAIN
    if allow_abstain:
        return ABSTAIN
    raise MetricsError("reference label is outside the frozen taxonomy")


def _label_map(value: Any, *, optional: bool = False) -> dict[str, str] | None:
    if value is None and optional:
        return None
    if not isinstance(value, dict):
        raise MetricsError("reference labels must be an object")
    result: dict[str, str] = {}
    for dimension in ("activity", "failure_category", "retry_loop", "anomaly", "phase"):
        if dimension not in value:
            if optional:
                return None
            raise MetricsError("reference labels are incomplete")
        result[dimension] = _label(value[dimension], dimension)
    return result


def _normalise_references(rows: Iterable[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    references: dict[str, dict[str, Any]] = {}
    session_metadata: dict[str, tuple[bool, int | None, str]] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise MetricsError("reference rows must be objects")
        event_id = row.get("id")
        session_id = row.get("session_id")
        harness = row.get("harness")
        event_order = row.get("event_order")
        if not isinstance(event_id, str) or not event_id or not isinstance(session_id, str) or not session_id:
            raise MetricsError("reference identity fields are incomplete")
        if not isinstance(harness, str) or harness not in {"claude", "codex"}:
            raise MetricsError("reference harness is invalid")
        if isinstance(event_order, bool) or not isinstance(event_order, int) or event_order < 0:
            raise MetricsError("reference event order is invalid")
        if event_id in references:
            raise MetricsError("reference event keys must be unique")
        gold = _label_map(row.get("gold"))
        causal_gold = _label_map(row.get("causal_gold"), optional=True)
        complete_session = row.get("complete_session", False)
        if not isinstance(complete_session, bool):
            raise MetricsError("complete-session marker is invalid")
        expected_count = row.get("eligible_event_count")
        if expected_count is not None and (isinstance(expected_count, bool) or not isinstance(expected_count, int) or expected_count < 1):
            raise MetricsError("eligible-event count is invalid")
        metadata = (complete_session, expected_count, harness)
        previous = session_metadata.setdefault(session_id, metadata)
        if previous != metadata:
            raise MetricsError("session coverage metadata is inconsistent")
        references[event_id] = {
            "session_id": session_id,
            "harness": harness,
            "event_order": event_order,
            "gold": gold,
            "causal_gold": causal_gold,
            "complete_session": complete_session,
            "eligible_event_count": expected_count,
        }
    if not references:
        raise MetricsError("reference input is empty")
    counts_by_session = Counter(item["session_id"] for item in references.values())
    orders_by_session: dict[str, set[int]] = defaultdict(set)
    for item in references.values():
        session_id = item["session_id"]
        if item["event_order"] in orders_by_session[session_id]:
            raise MetricsError("event order must be unique within a session")
        orders_by_session[session_id].add(item["event_order"])
    for session_id, (is_complete, expected_count, _) in session_metadata.items():
        if is_complete and (expected_count is None or expected_count != counts_by_session[session_id]):
            raise MetricsError("complete-session eligible-event coverage is inconsistent")
    return references


def _prediction_condition(row: dict[str, Any]) -> tuple[str, str]:
    condition = condition_name(row.get("strategy"), row.get("mode"))
    context = row.get("context")
    if not isinstance(context, str) or context not in CONTEXTS:
        raise MetricsError("prediction context is invalid")
    key = (condition, context)
    if key not in OPERATIONS_CONDITIONS:
        raise MetricsError("unsupported condition context")
    return key


def _normalise_predictions(rows: Iterable[dict[str, Any]], references: dict[str, dict[str, Any]]) -> tuple[dict[tuple[str, str], dict[str, dict[int, dict[str, Any]]]], dict[tuple[str, str], int]]:
    grouped: dict[tuple[str, str], dict[str, dict[int, dict[str, Any]]]] = defaultdict(lambda: defaultdict(dict))
    row_counts: Counter[tuple[str, str]] = Counter()
    for row in rows:
        if not isinstance(row, dict):
            raise MetricsError("prediction rows must be objects")
        event_id = row.get("id")
        repetition = row.get("repetition")
        if not isinstance(event_id, str) or event_id not in references:
            raise MetricsError("prediction event key is not in the reference")
        if isinstance(repetition, bool) or not isinstance(repetition, int) or repetition < 1:
            raise MetricsError("prediction repetition is invalid")
        key = _prediction_condition(row)
        predictions = row.get("predictions")
        if not isinstance(predictions, dict):
            raise MetricsError("prediction values must be an object")
        if repetition in grouped[key][event_id]:
            raise MetricsError("condition has duplicate event repetitions")
        grouped[key][event_id][repetition] = row
        row_counts[key] += 1
    if not grouped:
        raise MetricsError("prediction input is empty")
    expected_events = set(references)
    for by_event in grouped.values():
        if set(by_event) != expected_events:
            raise MetricsError("condition event coverage does not match the reference")
        repetition_sets = {tuple(sorted(repetitions)) for repetitions in by_event.values()}
        if len(repetition_sets) != 1:
            raise MetricsError("condition repetition coverage is inconsistent")
    return grouped, row_counts


def _normalise_summaries(rows: Iterable[dict[str, Any]]) -> dict[tuple[str, str], list[dict[str, Any]]]:
    grouped: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        if not isinstance(row, dict):
            raise MetricsError("summary rows must be objects")
        key = _prediction_condition(row)
        grouped[key].append(row)
    return grouped


def _normalise_operations(value: Any) -> dict[str, dict[str, Any]] | None:
    if value is None:
        return None
    schema = value.get("schema") if isinstance(value, dict) else None
    if not isinstance(schema, int) or isinstance(schema, bool) or schema != 1:
        raise MetricsError("operations aggregate schema is invalid")
    conditions = value.get("conditions")
    if not isinstance(conditions, dict):
        raise MetricsError("operations aggregate conditions are invalid")
    allowed = set(OPERATIONS_CONDITIONS.values())
    result: dict[str, dict[str, Any]] = {}
    for condition, row in conditions.items():
        if condition not in allowed or not isinstance(row, dict):
            raise MetricsError("operations aggregate condition is invalid")
        result[condition] = row
    return result


def _protocol_conformance(
    predictions: dict[tuple[str, str], dict[str, dict[int, dict[str, Any]]]],
    draws: int,
    seed: int,
) -> dict[str, Any]:
    repetition_checks: dict[str, dict[str, Any]] = {}
    repetition_ready = True
    for key, expected in EXPECTED_REPETITIONS.items():
        condition, context = key
        by_event = predictions.get(key)
        observed = sorted(next(iter(by_event.values()))) if by_event else []
        matches = observed == list(expected)
        repetition_ready = repetition_ready and matches
        repetition_checks[f"{condition}|{context}"] = {
            "expected": list(expected),
            "observed": observed,
            "matches": matches,
        }
    unexpected_conditions = sorted(
        f"{condition}|{context}"
        for condition, context in predictions
        if (condition, context) not in EXPECTED_REPETITIONS
    )
    draws_match = draws == DEFAULT_DRAWS
    seed_match = seed == DEFAULT_SEED
    conditions_match = not unexpected_conditions
    return {
        "status": "pass" if draws_match and seed_match and repetition_ready and conditions_match else "insufficient_evidence",
        "expected_bootstrap_draws": DEFAULT_DRAWS,
        "observed_bootstrap_draws": draws,
        "bootstrap_draws_match": draws_match,
        "expected_bootstrap_seed": DEFAULT_SEED,
        "observed_bootstrap_seed": seed,
        "bootstrap_seed_match": seed_match,
        "unexpected_conditions": unexpected_conditions,
        "repetition_sets": repetition_checks,
    }


def _prediction_label(row: dict[str, Any], dimension: str) -> str:
    value = row["predictions"].get(dimension)
    return _label(value, dimension, allow_abstain=True)


def _event_contributions(
    event_ids: Iterable[str],
    row_by_event: dict[str, dict[int, dict[str, Any]]],
    references: dict[str, dict[str, Any]],
    gold_field: str,
) -> dict[str, dict[str, Any]]:
    contributions: dict[str, dict[str, Any]] = {}
    for event_id in event_ids:
        trial_rows = row_by_event[event_id]
        if not trial_rows:
            continue
        gold = references[event_id][gold_field]
        if gold is None:
            continue
        counts: dict[str, Counter[tuple[str, str]]] = {}
        ordered_rows = [trial_rows[rep] for rep in sorted(trial_rows)]
        repetitions = len(ordered_rows)
        for dimension, labels in DIMENSIONS.items():
            if dimension == "failure_detection":
                continue
            truth = gold[dimension]
            prediction_counts: Counter[str] = Counter(_prediction_label(row, dimension) for row in ordered_rows)
            counts[dimension] = Counter({(truth, label): count / repetitions for label, count in prediction_counts.items()})

        truth_failure = "no_failure" if gold["failure_category"] == "none" else "failure"
        binary_predictions = []
        for row in ordered_rows:
            category = _prediction_label(row, "failure_category")
            binary_predictions.append(ABSTAIN if category == ABSTAIN else ("no_failure" if category == "none" else "failure"))
        binary_counts: Counter[str] = Counter(binary_predictions)
        counts["failure_detection"] = Counter({(truth_failure, label): count / repetitions for label, count in binary_counts.items()})
        contributions[event_id] = counts
    return contributions


def _confusion(
    contributions: dict[str, dict[str, Any]],
    weights: Counter[str],
    dimension: str,
) -> Counter[tuple[str, str]]:
    result: Counter[tuple[str, str]] = Counter()
    for event_id, weight in weights.items():
        if weight <= 0 or event_id not in contributions:
            continue
        for pair, fraction in contributions[event_id][dimension].items():
            result[pair] += weight * fraction
    return result


def _metric_values(confusion: Counter[tuple[str, str]], labels: tuple[str, ...]) -> dict[str, Any]:
    predicted_labels = (*labels, ABSTAIN)
    total = sum(confusion.values())
    correct = sum(confusion[(label, label)] for label in labels)
    per_class: dict[str, dict[str, float]] = {}
    for label in labels:
        true_positive = confusion[(label, label)]
        predicted_positive = sum(confusion[(actual, label)] for actual in labels)
        support = sum(confusion[(label, predicted)] for predicted in predicted_labels)
        precision = true_positive / predicted_positive if predicted_positive else 0.0
        recall = true_positive / support if support else 0.0
        f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
        per_class[label] = {
            "precision": precision,
            "recall": recall,
            "f1": f1,
            "support": support,
            "predicted": predicted_positive,
            "true_positive": true_positive,
        }
    class_count = len(labels) or 1
    return {
        "accuracy": correct / total if total else 0.0,
        "macro_precision": sum(item["precision"] for item in per_class.values()) / class_count,
        "macro_recall": sum(item["recall"] for item in per_class.values()) / class_count,
        "macro_f1": sum(item["f1"] for item in per_class.values()) / class_count,
        "per_class": per_class,
        "count": total,
    }


def _flatten_stats(values: dict[str, Any]) -> dict[str, float]:
    flattened = {key: float(values[key]) for key in ("accuracy", "macro_precision", "macro_recall", "macro_f1")}
    for label, item in values["per_class"].items():
        for metric in ("precision", "recall", "f1"):
            flattened[f"per_class|{label}|{metric}"] = float(item[metric])
    return flattened


def _percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * fraction
    low = math.floor(position)
    high = math.ceil(position)
    if low == high:
        return ordered[low]
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def _ci(values: list[float]) -> dict[str, float | None]:
    return {"lower": _percentile(values, (1 - CONFIDENCE) / 2), "upper": _percentile(values, (1 + CONFIDENCE) / 2)}


def _sampled_weights(units: list[str], unit_to_events: dict[str, list[str]], rng: random.Random) -> Counter[str]:
    sampled = rng.choices(units, k=len(units))
    unit_counts = Counter(sampled)
    event_counts: Counter[str] = Counter()
    for unit, weight in unit_counts.items():
        for event_id in unit_to_events[unit]:
            event_counts[event_id] += weight
    return event_counts


def _metric_bootstrap(
    contributions: dict[str, dict[str, Any]],
    event_ids: list[str],
    references: dict[str, dict[str, Any]],
    dimension: str,
    draws: int,
    seed: int,
    checkpoint: Callable[[], Any] | None,
) -> tuple[dict[str, Any], dict[str, list[float]], dict[str, list[float]]]:
    labels = DIMENSIONS[dimension]
    weights = Counter({event_id: 1 for event_id in event_ids})
    point = _metric_values(_confusion(contributions, weights, dimension), labels)
    values_event: dict[str, list[float]] = defaultdict(list)
    values_session: dict[str, list[float]] = defaultdict(list)

    rng_event = random.Random(seed)
    for draw in range(1, draws + 1):
        sampled = Counter(rng_event.choices(event_ids, k=len(event_ids)))
        metric = _metric_values(_confusion(contributions, sampled, dimension), labels)
        for key, value in _flatten_stats(metric).items():
            values_event[key].append(value)
        if checkpoint and draw % 25 == 0:
            checkpoint()

    session_to_events: dict[str, list[str]] = defaultdict(list)
    for event_id in event_ids:
        session_to_events[references[event_id]["session_id"]].append(event_id)
    session_units = sorted(session_to_events)
    rng_session = random.Random(seed)
    for draw in range(1, draws + 1):
        sampled = _sampled_weights(session_units, session_to_events, rng_session)
        metric = _metric_values(_confusion(contributions, sampled, dimension), labels)
        for key, value in _flatten_stats(metric).items():
            values_session[key].append(value)
        if checkpoint and draw % 25 == 0:
            checkpoint()
    return point, values_event, values_session


def _metric_output(
    point: dict[str, Any],
    event_samples: dict[str, list[float]],
    session_samples: dict[str, list[float]],
    unique_positive_counts: Counter[str],
) -> dict[str, Any]:
    def metric(key: str, denominator: float | None = None) -> dict[str, Any]:
        return {
            "estimate": point["accuracy"] if key == "accuracy" else point["macro_" + key.removeprefix("macro_")] if key.startswith("macro_") else point["per_class"][key.split("|")[1]][key.split("|")[2]],
            "event_bootstrap_ci95": _ci(event_samples.get(key, [])),
            "session_cluster_sensitivity_ci95": _ci(session_samples.get(key, [])),
            **({"denominator": denominator} if denominator is not None else {}),
        }

    result: dict[str, Any] = {
        "event_count": int(round(point["count"])),
        "accuracy": metric("accuracy", int(round(point["count"]))),
        "macro_precision": metric("macro_precision", int(round(point["count"]))),
        "macro_recall": metric("macro_recall", int(round(point["count"]))),
        "macro_f1": metric("macro_f1", int(round(point["count"]))),
        "per_class": {},
    }
    for label, values in point["per_class"].items():
        result["per_class"][label] = {
            "positive_events": unique_positive_counts[label],
            "predicted_positive_events": values["predicted"],
            "true_positive_events": values["true_positive"],
            "precision": metric(f"per_class|{label}|precision", values["predicted"]),
            "recall": metric(f"per_class|{label}|recall", int(round(values["support"]))),
            "f1": metric(f"per_class|{label}|f1"),
        }
    return result


def _score_metrics(
    references: dict[str, dict[str, Any]],
    row_by_event: dict[str, dict[int, dict[str, Any]]],
    row_count: int,
    gold_field: str,
    draws: int,
    seed: int,
    checkpoint: Callable[[], Any] | None,
) -> tuple[dict[str, Any], dict[str, dict[str, Any]], list[str]]:
    event_ids = sorted(event_id for event_id in row_by_event if references[event_id][gold_field] is not None)
    if not event_ids:
        return {"event_count": 0, "session_count": 0, "prediction_rows": row_count, "dimensions": {}}, {}, []
    contributions = _event_contributions(event_ids, row_by_event, references, gold_field)
    metrics: dict[str, Any] = {}
    point_by_dimension: dict[str, dict[str, Any]] = {}
    for index, dimension in enumerate(DIMENSIONS):
        point, event_samples, session_samples = _metric_bootstrap(
            contributions, event_ids, references, dimension, draws, seed + index, checkpoint,
        )
        point_by_dimension[dimension] = point
        if dimension == "failure_detection":
            gold_labels = [
                "no_failure" if references[event_id][gold_field]["failure_category"] == "none" else "failure"
                for event_id in event_ids
            ]
        else:
            gold_labels = [references[event_id][gold_field][dimension] for event_id in event_ids]
        positives = Counter(gold_labels)
        metrics[dimension] = _metric_output(point, event_samples, session_samples, positives)
    result = {
        "event_count": len(event_ids),
        "session_count": len({references[event_id]["session_id"] for event_id in event_ids}),
        "prediction_rows": row_count,
        "dimensions": metrics,
    }
    return result, point_by_dimension, event_ids


def _harness_descriptive_points(
    references: dict[str, dict[str, Any]],
    row_by_event: dict[str, dict[int, dict[str, Any]]],
    gold_field: str,
) -> dict[str, Any]:
    event_ids = sorted(
        event_id for event_id in row_by_event
        if references[event_id][gold_field] is not None
    )
    contributions = _event_contributions(event_ids, row_by_event, references, gold_field)
    result: dict[str, Any] = {
        "estimate_type": "descriptive_point_estimate",
        "confidence_intervals": "not_computed",
        "bootstrap_scope": "pooled_condition_context_metrics_only",
        "harness_role_confounded": True,
    }
    for harness in ("claude", "codex"):
        stratum_ids = [
            event_id for event_id in event_ids
            if references[event_id]["harness"] == harness
        ]
        stratum: dict[str, Any] = {
            "unique_event_count": len(stratum_ids),
            "session_count": len({references[event_id]["session_id"] for event_id in stratum_ids}),
            "dimensions": {},
        }
        if stratum_ids:
            weights = Counter({event_id: 1 for event_id in stratum_ids})
            for dimension, labels in DIMENSIONS.items():
                point = _metric_values(_confusion(contributions, weights, dimension), labels)
                stratum["dimensions"][dimension] = {
                    "accuracy": point["accuracy"],
                    "macro_precision": point["macro_precision"],
                    "macro_recall": point["macro_recall"],
                    "macro_f1": point["macro_f1"],
                    "per_class": {
                        label: {
                            "precision": values["precision"],
                            "recall": values["recall"],
                            "f1": values["f1"],
                            "positive_events": int(round(values["support"])),
                        }
                        for label, values in point["per_class"].items()
                    },
                }
        result[harness] = stratum
    return result


def _paired_delta(
    references: dict[str, dict[str, Any]],
    left_rows: dict[str, dict[int, dict[str, Any]]],
    right_rows: dict[str, dict[int, dict[str, Any]]],
    draws: int,
    seed: int,
    checkpoint: Callable[[], Any] | None,
) -> dict[str, Any] | None:
    ids = sorted(set(left_rows).intersection(right_rows))
    if not ids:
        return None
    left = _event_contributions(ids, left_rows, references, "gold")
    right = _event_contributions(ids, right_rows, references, "gold")

    def score(contributions: dict[str, dict[str, Any]], weights: Counter[str]) -> float:
        return _metric_values(_confusion(contributions, weights, "activity"), DIMENSIONS["activity"])["macro_f1"]

    unit = Counter({event_id: 1 for event_id in ids})
    estimate = score(left, unit) - score(right, unit)
    event_draws: list[float] = []
    rng = random.Random(seed)
    for draw in range(1, draws + 1):
        weights = Counter(rng.choices(ids, k=len(ids)))
        event_draws.append(score(left, weights) - score(right, weights))
        if checkpoint and draw % 25 == 0:
            checkpoint()

    sessions: dict[str, list[str]] = defaultdict(list)
    for event_id in ids:
        sessions[references[event_id]["session_id"]].append(event_id)
    session_ids = sorted(sessions)
    rng = random.Random(seed)
    session_draws: list[float] = []
    for draw in range(1, draws + 1):
        weights = _sampled_weights(session_ids, sessions, rng)
        session_draws.append(score(left, weights) - score(right, weights))
        if checkpoint and draw % 25 == 0:
            checkpoint()
    class_support_counts = Counter(references[event_id]["gold"]["activity"] for event_id in ids)
    class_support = {label: class_support_counts[label] for label in DIMENSIONS["activity"]}
    return {
        "event_count": len(ids),
        "session_count": len(session_ids),
        "activity_positive_events": dict(class_support),
        "estimate": estimate,
        "event_bootstrap_ci95": _ci(event_draws),
        "session_cluster_sensitivity_ci95": _ci(session_draws),
    }


def _boundary_metrics(
    references: dict[str, dict[str, Any]],
    row_by_event: dict[str, dict[int, dict[str, Any]]],
) -> dict[str, Any]:
    selected = sorted(row_by_event, key=lambda event_id: (references[event_id]["session_id"], references[event_id]["event_order"]))
    per_session: dict[str, list[str]] = defaultdict(list)
    for event_id in selected:
        per_session[references[event_id]["session_id"]].append(event_id)
    tp = fp = fn = covered = pairs = 0.0
    for event_ids in per_session.values():
        event_ids.sort(key=lambda event_id: references[event_id]["event_order"])
        for left_id, right_id in zip(event_ids, event_ids[1:]):
            shared_repetitions = sorted(set(row_by_event[left_id]).intersection(row_by_event[right_id]))
            if not shared_repetitions:
                continue
            gold_boundary = references[left_id]["gold"]["phase"] != references[right_id]["gold"]["phase"]
            pair_tp = pair_fp = pair_fn = pair_covered = 0
            for repetition in shared_repetitions:
                left = _prediction_label(row_by_event[left_id][repetition], "phase")
                right = _prediction_label(row_by_event[right_id][repetition], "phase")
                is_covered = left != ABSTAIN and right != ABSTAIN
                predicted_boundary = is_covered and left != right
                pair_covered += int(is_covered)
                pair_tp += int(gold_boundary and predicted_boundary)
                pair_fp += int(not gold_boundary and predicted_boundary)
                pair_fn += int(gold_boundary and not predicted_boundary)
            divisor = len(shared_repetitions)
            tp += pair_tp / divisor
            fp += pair_fp / divisor
            fn += pair_fn / divisor
            covered += pair_covered / divisor
            pairs += 1
    precision = tp / (tp + fp) if tp + fp else 0.0
    recall = tp / (tp + fn) if tp + fn else 0.0
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return {
        "pair_count": int(pairs),
        "covered_pair_equivalents": covered,
        "true_positive_equivalents": tp,
        "false_positive_equivalents": fp,
        "false_negative_equivalents": fn,
        "precision": precision,
        "recall": recall,
        "f1": f1,
    }


def _session_alert_budget(
    references: dict[str, dict[str, Any]],
    row_by_event: dict[str, dict[int, dict[str, Any]]],
    checkpoint: Callable[[], Any] | None,
    draws: int,
    seed: int,
) -> dict[str, Any]:
    session_to_ids: dict[str, list[str]] = defaultdict(list)
    for event_id, ref in references.items():
        if ref["complete_session"]:
            session_to_ids[ref["session_id"]].append(event_id)
    if not session_to_ids:
        return {"status": "no_complete_scored_sessions", "session_count": 0, "covered_session_count": 0}
    covered_sessions: dict[str, list[str]] = {}
    for session_id, event_ids in session_to_ids.items():
        if all(event_id in row_by_event for event_id in event_ids):
            covered_sessions[session_id] = event_ids
    if len(covered_sessions) != len(session_to_ids):
        return {
            "status": "incomplete_prediction_coverage",
            "session_count": len(session_to_ids),
            "covered_session_count": len(covered_sessions),
        }

    mean_alerts_by_session: dict[str, float] = {}
    over_budget_by_session: dict[str, float] = {}
    for session_id, event_ids in covered_sessions.items():
        repetition_sets = [set(row_by_event[event_id]) for event_id in event_ids]
        common_repetitions = sorted(set.intersection(*repetition_sets)) if repetition_sets else []
        if not common_repetitions or any(reps != set(common_repetitions) for reps in repetition_sets):
            return {
                "status": "incomplete_prediction_repetitions",
                "session_count": len(session_to_ids),
                "covered_session_count": len(covered_sessions),
            }
        alert_counts: list[int] = []
        for repetition in common_repetitions:
            alert_counts.append(sum(
                references[event_id]["gold"]["anomaly"] == "false"
                and _prediction_label(row_by_event[event_id][repetition], "anomaly") == "true"
                for event_id in event_ids
            ))
        mean_alerts_by_session[session_id] = sum(alert_counts) / len(alert_counts)
        over_budget_by_session[session_id] = float(mean_alerts_by_session[session_id] > 1)

    sessions = sorted(mean_alerts_by_session)
    estimate_alerts = sum(mean_alerts_by_session[session_id] for session_id in sessions) / len(sessions)
    estimate_fraction = sum(over_budget_by_session[session_id] for session_id in sessions) / len(sessions)
    rng = random.Random(seed)
    alert_draws: list[float] = []
    fraction_draws: list[float] = []
    for draw in range(1, draws + 1):
        sampled = rng.choices(sessions, k=len(sessions))
        alert_draws.append(sum(mean_alerts_by_session[session_id] for session_id in sampled) / len(sessions))
        fraction_draws.append(sum(over_budget_by_session[session_id] for session_id in sampled) / len(sessions))
        if checkpoint and draw % 25 == 0:
            checkpoint()
    return {
        "status": "measured",
        "session_count": len(sessions),
        "covered_session_count": len(sessions),
        "mean_false_positive_alerts_per_session": {
            "estimate": estimate_alerts,
            "session_bootstrap_ci95": _ci(alert_draws),
            "denominator": len(sessions),
        },
        "fraction_sessions_over_one_false_positive_alert": {
            "estimate": estimate_fraction,
            "session_bootstrap_ci95": _ci(fraction_draws),
            "denominator": len(sessions),
        },
    }


def _operation_metrics(
    summary_rows: list[dict[str, Any]],
    operations_row: dict[str, Any] | None,
    *,
    expected_event_count: int,
    expected_prediction_rows: int,
) -> dict[str, Any]:
    request_totals: list[int | None] = []
    failure_totals: list[int | None] = []
    input_token_totals: list[int | None] = []
    output_token_totals: list[int | None] = []
    jev_estimates: list[float | None] = []
    http_p95s: list[float] = []
    provider_latency_incomplete = False
    projected_p95s: list[float] = []
    projected_latency_incomplete = False
    unknown_wait_rows = 0
    unknown_wait_count_known = bool(summary_rows)

    def count_value(value: Any, message: str) -> int | None:
        if value is None:
            return None
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise MetricsError(message)
        return value

    def run_count(nodes: list[dict[str, Any]], row: dict[str, Any], field: str, message: str) -> int | None:
        values = [count_value(node.get(field), message) for node in nodes]
        if all(value is not None for value in values):
            return sum(value for value in values if value is not None)
        # real_run writes an aggregate as well; use it when a session summary
        # lacks a count instead of treating missing data as zero.
        return count_value(row.get(field), message)

    def run_usage_total(nodes: list[dict[str, Any]], row: dict[str, Any], field: str) -> int | None:
        values = []
        for node in nodes:
            usage = node.get("usage") if isinstance(node.get("usage"), dict) else {}
            value = usage.get(field)
            values.append(value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else None)
        if all(value is not None for value in values):
            return sum(value for value in values if value is not None)
        usage = row.get("usage") if isinstance(row.get("usage"), dict) else {}
        value = usage.get(field)
        return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else None

    def run_jev_estimate(nodes: list[dict[str, Any]], row: dict[str, Any]) -> float | None:
        estimates: list[float | None] = []
        for node in nodes:
            cost = node.get("cost") if isinstance(node.get("cost"), dict) else {}
            estimate = cost.get("estimate_usd")
            if cost.get("basis") == "published_list_price_estimate" and isinstance(estimate, (int, float)) and not isinstance(estimate, bool) and math.isfinite(estimate):
                estimates.append(float(estimate))
            else:
                estimates.append(None)
        if all(value is not None for value in estimates):
            return sum(value for value in estimates if value is not None)
        cost = row.get("cost") if isinstance(row.get("cost"), dict) else {}
        estimate = cost.get("estimate_usd")
        if cost.get("basis") == "published_list_price_estimate" and isinstance(estimate, (int, float)) and not isinstance(estimate, bool) and math.isfinite(estimate):
            return float(estimate)
        return None

    for row in summary_rows:
        if "sessions" in row:
            sessions = row.get("sessions")
            if not isinstance(sessions, list) or not sessions:
                raise MetricsError("summary session telemetry is invalid")
            if any(not isinstance(session, dict) or not isinstance(session.get("summary"), dict) for session in sessions):
                raise MetricsError("summary session telemetry is invalid")
            nodes = [session["summary"] for session in sessions]
        else:
            nodes = [row]

        request_count = run_count(nodes, row, "request_count", "summary request denominator is invalid")
        failure_count = run_count(nodes, row, "request_failure_count", "summary request-failure count is invalid")
        if request_count is not None and failure_count is not None and failure_count > request_count:
            raise MetricsError("summary request-failure count is invalid")
        request_totals.append(request_count)
        failure_totals.append(failure_count)
        input_token_totals.append(run_usage_total(nodes, row, "input_tokens"))
        output_token_totals.append(run_usage_total(nodes, row, "output_tokens"))
        jev_estimates.append(run_jev_estimate(nodes, row))

        for node in nodes:
            latency = node.get("latency") if isinstance(node.get("latency"), dict) else {}
            http = latency.get("provider_http_rtt") if isinstance(latency.get("provider_http_rtt"), dict) else {}
            http_p95 = http.get("p95_ms")
            if isinstance(http_p95, (int, float)) and not isinstance(http_p95, bool) and math.isfinite(http_p95):
                http_p95s.append(float(http_p95))
            else:
                node_requests = count_value(node.get("request_count"), "summary request denominator is invalid")
                if node_requests != 0:
                    provider_latency_incomplete = True

            projected = latency.get("projected_online_event_latency") if isinstance(latency.get("projected_online_event_latency"), dict) else {}
            projected_p95 = projected.get("p95_ms")
            if isinstance(projected_p95, (int, float)) and not isinstance(projected_p95, bool) and math.isfinite(projected_p95):
                projected_p95s.append(float(projected_p95))
            else:
                projected_latency_incomplete = True

            unknown = count_value(latency.get("queue_wait_unknown_event_rows"), "summary unknown-wait count is invalid")
            if unknown is None:
                unknown_wait_count_known = False
            else:
                unknown_wait_rows += unknown

    summary_requests = (
        sum(value for value in request_totals if value is not None)
        if request_totals and all(value is not None for value in request_totals) else None
    )
    summary_failures = (
        sum(value for value in failure_totals if value is not None)
        if failure_totals and all(value is not None for value in failure_totals) else None
    )
    pooled_http_p95 = None
    pooled_projected_p95 = None
    request_unknown_count = None
    operations_wait_unknown = None
    if operations_row is not None:
        if operations_row.get("status") != "measured":
            raise MetricsError("operations aggregate status is invalid")
        operation_events = count_value(operations_row.get("event_count"), "operations event count is invalid")
        operation_rows = count_value(operations_row.get("prediction_rows"), "operations prediction count is invalid")
        requests = count_value(operations_row.get("request_count"), "operations request count is invalid")
        failures = count_value(operations_row.get("failed_request_count"), "operations failure count is invalid")
        if any(value is None for value in (operation_events, operation_rows, requests, failures)):
            raise MetricsError("operations aggregate counts are incomplete")
        if operation_events != expected_event_count or operation_rows != expected_prediction_rows:
            raise MetricsError("operations event coverage does not match predictions")
        if failures > requests:
            raise MetricsError("operations request-failure count is invalid")
        if summary_requests is not None and summary_requests != requests:
            raise MetricsError("operations request count does not match summaries")
        if summary_failures is not None and summary_failures != failures:
            raise MetricsError("operations failure count does not match summaries")

        rtt = operations_row.get("request_rtt_ms")
        if not isinstance(rtt, dict):
            raise MetricsError("operations request latency is invalid")
        rtt_count = count_value(rtt.get("count"), "operations request latency count is invalid")
        request_unknown_count = count_value(rtt.get("unknown_count"), "operations unknown request latency count is invalid")
        if rtt_count is None or request_unknown_count is None:
            raise MetricsError("operations request latency counts are incomplete")
        if rtt_count + request_unknown_count != requests:
            raise MetricsError("operations request latency coverage is inconsistent")
        rtt_p95 = rtt.get("p95_ms")
        if rtt_p95 is not None and (isinstance(rtt_p95, bool) or not isinstance(rtt_p95, (int, float)) or not math.isfinite(rtt_p95) or rtt_p95 < 0):
            raise MetricsError("operations request latency percentile is invalid")
        if (rtt_count == 0) != (rtt_p95 is None):
            raise MetricsError("operations request latency percentile coverage is inconsistent")
        if request_unknown_count == 0:
            pooled_http_p95 = float(rtt_p95) if rtt_p95 is not None else None

        latency = operations_row.get("latency_ms")
        if not isinstance(latency, dict):
            raise MetricsError("operations event latency is invalid")
        projected = latency.get("projected_online_latency_ms")
        queue_wait = latency.get("logical_queue_wait_ms")
        if not isinstance(projected, dict) or not isinstance(queue_wait, dict):
            raise MetricsError("operations event latency is invalid")
        projected_count = count_value(projected.get("count"), "operations projected latency count is invalid")
        projected_unknown = count_value(projected.get("unknown_rows"), "operations unknown projected latency count is invalid")
        wait_count = count_value(queue_wait.get("count"), "operations queue-wait count is invalid")
        operations_wait_unknown = count_value(queue_wait.get("unknown_rows"), "operations unknown-wait count is invalid")
        if any(value is None for value in (projected_count, projected_unknown, wait_count, operations_wait_unknown)):
            raise MetricsError("operations event latency counts are incomplete")
        if projected_count + projected_unknown != operation_rows or wait_count + operations_wait_unknown != operation_rows:
            raise MetricsError("operations event latency coverage is inconsistent")
        projected_p95 = projected.get("p95_ms")
        if projected_p95 is not None and (isinstance(projected_p95, bool) or not isinstance(projected_p95, (int, float)) or not math.isfinite(projected_p95) or projected_p95 < 0):
            raise MetricsError("operations projected latency percentile is invalid")
        if (projected_count == 0) != (projected_p95 is None):
            raise MetricsError("operations projected latency percentile coverage is inconsistent")
        if operations_wait_unknown == 0 and projected_unknown == 0:
            pooled_projected_p95 = float(projected_p95) if projected_p95 is not None else None

        if summary_rows and unknown_wait_count_known and unknown_wait_rows != operations_wait_unknown:
            raise MetricsError("operations unknown-wait count does not match summaries")
        unknown_wait_rows = operations_wait_unknown
        unknown_wait_count_known = True
    else:
        requests = summary_requests
        failures = summary_failures

    if requests is not None and failures is not None and failures > requests:
        raise MetricsError("summary request-failure count is invalid")
    return {
        "status": "measured" if operations_row is not None or (requests or 0) or http_p95s or projected_p95s else "not_provided" if not summary_rows else "not_measured",
        "summary_run_count": len(summary_rows),
        "request_count": requests,
        "request_failure_count": failures,
        "request_failure_rate": failures / requests if requests and failures is not None else None,
        "provider_http_p95_ms_worst_session": max(http_p95s) if http_p95s and not provider_latency_incomplete else None,
        "provider_http_p95_ms_pooled": pooled_http_p95,
        "projected_async_p95_ms_worst_session": max(projected_p95s) if projected_p95s and not projected_latency_incomplete and unknown_wait_count_known and unknown_wait_rows == 0 else None,
        "projected_async_p95_ms_pooled": pooled_projected_p95,
        "queue_wait_unknown_event_rows": unknown_wait_rows if unknown_wait_count_known else operations_wait_unknown,
        "request_latency_unknown_count": request_unknown_count,
        "input_tokens": sum(value for value in input_token_totals if value is not None) if input_token_totals and all(value is not None for value in input_token_totals) else None,
        "output_tokens": sum(value for value in output_token_totals if value is not None) if output_token_totals and all(value is not None for value in output_token_totals) else None,
        "billed_cost_usd": None,
        "haiku_billing_units": None,
        "billing_status": "unknown",
        "jev_list_estimate_usd": sum(value for value in jev_estimates if value is not None) if jev_estimates and all(value is not None for value in jev_estimates) else None,
    }


def _agreement(
    references: dict[str, dict[str, Any]],
    annotations_a: dict[str, dict[str, Any]] | None,
    annotations_b: dict[str, dict[str, Any]] | None,
    field: str,
) -> dict[str, Any]:
    if annotations_a is None or annotations_b is None:
        return {"status": "not_provided", "event_count": 0, "dimensions": {}}
    ids = sorted(set(annotations_a).intersection(annotations_b).intersection(references))
    result: dict[str, Any] = {"status": "computed" if ids else "no_overlap", "event_count": len(ids), "dimensions": {}}
    for dimension in ("activity", "failure_category", "retry_loop", "anomaly", "phase"):
        paired: list[tuple[str, str]] = []
        for event_id in ids:
            left = annotations_a[event_id].get(field)
            right = annotations_b[event_id].get(field)
            if left is None or right is None:
                continue
            paired.append((left[dimension], right[dimension]))
        count = len(paired)
        agreements = sum(left == right for left, right in paired)
        observed = agreements / count if count else None
        labels = DIMENSIONS[dimension]
        left_dist = Counter(left for left, _ in paired)
        right_dist = Counter(right for _, right in paired)
        expected = sum((left_dist[label] / count) * (right_dist[label] / count) for label in labels) if count else None
        kappa = (observed - expected) / (1 - expected) if count and expected is not None and expected < 1 else None
        result["dimensions"][dimension] = {
            "event_count": count,
            "agreement_count": agreements,
            "disagreement_count": count - agreements,
            "raw_agreement": observed,
            "cohen_kappa": kappa,
        }
    return result


def _normalise_annotation_rows(rows: Iterable[dict[str, Any]]) -> dict[str, dict[str, dict[str, str] | None]]:
    annotations: dict[str, dict[str, dict[str, str] | None]] = {}
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("id"), str) or not row["id"] or row["id"] in annotations:
            raise MetricsError("annotation rows are invalid")
        annotations[row["id"]] = {
            "gold": _label_map(row.get("gold"), optional=True),
            "causal_gold": _label_map(row.get("causal_gold"), optional=True),
        }
    return annotations


def _criterion(
    value: float | None,
    threshold: float,
    *,
    lower: bool = False,
    upper: bool = False,
    positive_events: int | None = None,
    positive_events_by_class: dict[str, int] | None = None,
) -> dict[str, Any]:
    result: dict[str, Any] = {"threshold": threshold}
    if positive_events_by_class is not None:
        result["unique_positive_events_by_class"] = positive_events_by_class
        result["minimum_unique_positive_events"] = 10
        if any(count < 10 for count in positive_events_by_class.values()):
            result["status"] = "insufficient_positive_support"
            return result
    if positive_events is not None:
        result["unique_positive_events"] = positive_events
        result["minimum_unique_positive_events"] = 10
        if positive_events < 10:
            result["status"] = "insufficient_positive_support"
            return result
    if value is None:
        result["status"] = "not_measured"
        return result
    result["bound"] = value
    passed = value >= threshold if lower else value <= threshold if upper else value > threshold
    result["status"] = "pass" if passed else "fail"
    return result


def _acceptance_criteria(
    condition: str,
    metrics: dict[str, Any],
    alert_budget: dict[str, Any],
    paired: dict[str, Any] | None,
    operations: dict[str, Any],
    *,
    population_ready: bool,
    adjudication_frozen: bool,
    independent_agreement_ready: bool,
    protocol_conformance_ready: bool,
) -> dict[str, Any]:
    if condition == "Rules":
        return {"status": "baseline", "criteria": {}}
    dimensions = metrics.get("dimensions", {})
    anomaly = dimensions.get("anomaly", {}).get("per_class", {}).get("true", {})
    failure = dimensions.get("failure_detection", {}).get("per_class", {}).get("failure", {})
    retry = dimensions.get("retry_loop", {}).get("per_class", {}).get("retry", {})
    loop = dimensions.get("retry_loop", {}).get("per_class", {}).get("loop", {})
    anomaly_ci = anomaly.get("precision", {}).get("event_bootstrap_ci95", {}).get("lower")
    failure_ci = failure.get("recall", {}).get("event_bootstrap_ci95", {}).get("lower")
    retry_ci = retry.get("recall", {}).get("event_bootstrap_ci95", {}).get("lower")
    loop_ci = loop.get("recall", {}).get("event_bootstrap_ci95", {}).get("lower")
    anomaly_support = anomaly.get("positive_events", 0)
    failure_support = failure.get("positive_events", 0)
    retry_support = retry.get("positive_events", 0)
    loop_support = loop.get("positive_events", 0)
    activity_support = paired.get("activity_positive_events") if paired else None
    fp_upper = alert_budget.get("mean_false_positive_alerts_per_session", {}).get("session_bootstrap_ci95", {}).get("upper") if alert_budget.get("status") == "measured" else None
    activity_lower = paired.get("event_bootstrap_ci95", {}).get("lower") if paired else None
    jev = "Jev" in condition
    http_limit = 1500.0 if jev else 10000.0
    criteria = {
        "anomaly_precision_lower95": _criterion(anomaly_ci, 0.80, lower=True, positive_events=anomaly_support),
        "false_alerts_per_session_upper95": _criterion(fp_upper, 1.0, upper=True, positive_events=anomaly_support),
        "any_failure_recall_lower95": _criterion(failure_ci, 0.85, lower=True, positive_events=failure_support),
        "retry_recall_lower95": _criterion(retry_ci, 0.80, lower=True, positive_events=retry_support),
        "loop_recall_lower95": _criterion(loop_ci, 0.80, lower=True, positive_events=loop_support),
        "activity_macro_f1_improvement_lower95": _criterion(
            activity_lower, 0.0, positive_events_by_class=activity_support,
        ),
        "provider_request_failure_rate": _criterion(operations.get("request_failure_rate"), 0.01, upper=True),
        "provider_http_p95_ms": _criterion(operations.get("provider_http_p95_ms_pooled"), http_limit, upper=True),
        "projected_async_p95_ms": _criterion(operations.get("projected_async_p95_ms_pooled"), 15000.0, upper=True),
    }
    statuses = {item["status"] for item in criteria.values()}
    gates = {
        "population_scope": "pass" if population_ready else "insufficient_evidence",
        "adjudication_frozen": "pass" if adjudication_frozen else "pending",
        "independent_annotation_agreement": "pass" if independent_agreement_ready else "insufficient_evidence",
        "protocol_conformance": "pass" if protocol_conformance_ready else "insufficient_evidence",
    }
    if not population_ready or not adjudication_frozen or not independent_agreement_ready or not protocol_conformance_ready:
        status = "insufficient_evidence"
    else:
        status = "pass" if statuses == {"pass"} else "fail" if "fail" in statuses else "insufficient_evidence"
    return {"status": status, "gates": gates, "criteria": criteria}


def score_records(
    reference_rows: Iterable[dict[str, Any]],
    prediction_rows: Iterable[dict[str, Any]],
    *,
    summary_rows: Iterable[dict[str, Any]] = (),
    operations: dict[str, Any] | None = None,
    annotation_a_rows: Iterable[dict[str, Any]] | None = None,
    annotation_b_rows: Iterable[dict[str, Any]] | None = None,
    user_review_rows: Iterable[dict[str, Any]] | None = None,
    adjudication_status: str = "pending",
    draws: int = DEFAULT_DRAWS,
    seed: int = DEFAULT_SEED,
    checkpoint: Callable[[], Any] | None = None,
) -> dict[str, Any]:
    if isinstance(draws, bool) or not isinstance(draws, int) or draws < 1:
        raise MetricsError("bootstrap draws must be positive")
    if not isinstance(seed, int) or isinstance(seed, bool):
        raise MetricsError("bootstrap seed must be an integer")
    if adjudication_status not in {"pending", "frozen"}:
        raise MetricsError("adjudication status is invalid")
    references = _normalise_references(reference_rows)
    predictions, row_counts = _normalise_predictions(prediction_rows, references)
    summaries = _normalise_summaries(summary_rows)
    operation_rows = _normalise_operations(operations)
    protocol_conformance = _protocol_conformance(predictions, draws, seed)
    if operation_rows is not None:
        prediction_operation_conditions = {
            OPERATIONS_CONDITIONS[key] for key in predictions
        }
        if set(operation_rows) - prediction_operation_conditions:
            raise MetricsError("operations condition is not in predictions")
    annotation_a = _normalise_annotation_rows(annotation_a_rows) if annotation_a_rows is not None else None
    annotation_b = _normalise_annotation_rows(annotation_b_rows) if annotation_b_rows is not None else None
    if (annotation_a is None) != (annotation_b is None):
        raise MetricsError("both independent annotation files are required")
    user_review = _normalise_annotation_rows(user_review_rows) if user_review_rows is not None else None

    sessions: dict[str, int] = Counter(item["session_id"] for item in references.values())
    complete_sessions = {item["session_id"] for item in references.values() if item["complete_session"]}
    harness_by_session = {item["session_id"]: item["harness"] for item in references.values()}
    session_counts_by_harness = Counter(harness_by_session.values())
    complete_counts_by_harness = Counter(
        harness_by_session[session_id] for session_id in complete_sessions
    )
    population_ready = (
        len(references) >= 300
        and len(complete_sessions) >= 15
        and len(complete_sessions) == len(sessions)
        and all(complete_counts_by_harness[harness] >= 5 for harness in ("claude", "codex"))
    )
    output_conditions: dict[str, dict[str, Any]] = defaultdict(dict)
    paired_activity: dict[str, Any] = {}
    paired_context: dict[str, Any] = {}
    acceptance: dict[str, Any] = {}
    condition_contexts = sorted(predictions, key=lambda key: (key[0], key[1]))

    for index, key in enumerate(condition_contexts):
        condition, context = key
        row_by_event = predictions[key]
        total_rows = row_counts[key]
        metrics, _, scored_ids = _score_metrics(
            references, row_by_event, total_rows, "gold", draws, seed + index * 31, checkpoint,
        )
        alert_budget = _session_alert_budget(references, row_by_event, checkpoint, draws, seed + index * 71)
        operations = _operation_metrics(
            summaries.get(key, []),
            operation_rows.get(OPERATIONS_CONDITIONS[key]) if operation_rows is not None else None,
            expected_event_count=len(references),
            expected_prediction_rows=total_rows,
        )
        score: dict[str, Any] = {
            "scored_event_count": len(scored_ids),
            "prediction_row_count": total_rows,
            "metrics": metrics,
            "harness_descriptive_points": _harness_descriptive_points(
                references, row_by_event, "gold",
            ),
            "phase_boundary": _boundary_metrics(references, row_by_event),
            "false_alert_budget": alert_budget,
            "operations": operations,
        }
        if context == "prefix":
            causal_metrics, _, causal_ids = _score_metrics(
                references, row_by_event, total_rows, "causal_gold", draws, seed + index * 31 + 1009, checkpoint,
            )
            score["causal_reference_metrics"] = causal_metrics if causal_ids else None
            score["causal_reference_harness_descriptive_points"] = (
                _harness_descriptive_points(references, row_by_event, "causal_gold")
                if causal_ids else None
            )
        output_conditions[condition][context] = score

    agreement = _agreement(references, annotation_a, annotation_b, "gold")
    if annotation_a is not None and annotation_b is not None:
        agreement["causal"] = _agreement(references, annotation_a, annotation_b, "causal_gold")

    def agreement_is_ready(scope: dict[str, Any] | None) -> bool:
        return bool(
            scope
            and scope.get("status") == "computed"
            and scope.get("event_count") == len(references)
            and set(scope.get("dimensions", {})) == set(DIMENSIONS) - {"failure_detection"}
            and all(item.get("event_count") == len(references) for item in scope.get("dimensions", {}).values())
        )

    full_annotation_coverage = bool(
        annotation_a is not None
        and annotation_b is not None
        and set(annotation_a) == set(references)
        and set(annotation_b) == set(references)
    )
    independent_agreement_ready = (
        full_annotation_coverage
        and agreement_is_ready(agreement)
        and agreement_is_ready(agreement.get("causal"))
    )

    for condition in PRIMARY_CONDITIONS:
        for context in ("prefix", "offline"):
            model = predictions.get((condition, context))
            baseline = predictions.get(("Rules", context))
            if model is None or baseline is None:
                continue
            paired = _paired_delta(references, model, baseline, draws, seed + 2003, checkpoint)
            if paired is not None:
                paired_activity.setdefault(condition, {})[context] = paired
        if condition in {"Jev per-event", "Haiku per-event"}:
            prefix_rows = predictions.get((condition, "prefix"))
            offline_rows = predictions.get((condition, "offline"))
            if prefix_rows is not None and offline_rows is not None:
                delta = _paired_delta(references, prefix_rows, offline_rows, draws, seed + 4001, checkpoint)
                if delta is not None:
                    paired_context[condition] = delta
        prefix = output_conditions.get(condition, {}).get("prefix")
        if prefix is not None:
            criteria = _acceptance_criteria(
                condition,
                prefix["metrics"],
                prefix["false_alert_budget"],
                paired_activity.get(condition, {}).get("prefix"),
                prefix["operations"],
                population_ready=population_ready,
                adjudication_frozen=adjudication_status == "frozen",
                independent_agreement_ready=independent_agreement_ready,
                protocol_conformance_ready=protocol_conformance["status"] == "pass",
            )
            acceptance[condition] = criteria

    rules_prefix = predictions.get(("Rules", "prefix"))
    rules_offline = predictions.get(("Rules", "offline"))
    if rules_prefix is not None and rules_offline is not None:
        rules_delta = _paired_delta(references, rules_prefix, rules_offline, draws, seed + 4001, checkpoint)
        if rules_delta is not None:
            paired_context["Rules"] = rules_delta

    review_result: dict[str, Any]
    if user_review is None:
        review_result = {"status": "pending", "event_count": 0, "dimensions": {}}
    else:
        review_result = _agreement(references, user_review, {event_id: {"gold": ref["gold"], "causal_gold": ref["causal_gold"]} for event_id, ref in references.items()}, "gold")
        review_result["status"] = "recorded" if review_result["event_count"] else "no_overlap"

    return {
        "schema_version": 1,
        "protocol_version": "asi-jev-real-v1",
        "bootstrap": {"confidence": CONFIDENCE, "draws": draws, "seed": seed, "event_unit": "unique_event", "repetitions_averaged_within_event": True},
        "protocol_conformance": protocol_conformance,
        "reference_counts": {
            "unique_events": len(references),
            "sessions": len(sessions),
            "complete_scored_sessions": len(complete_sessions),
            "sessions_by_harness": {harness: session_counts_by_harness[harness] for harness in ("claude", "codex")},
            "complete_scored_sessions_by_harness": {harness: complete_counts_by_harness[harness] for harness in ("claude", "codex")},
        },
        "population_evidence": {
            "status": "sufficient" if population_ready else "insufficient",
            "unique_events": len(references),
            "minimum_unique_events": 300,
            "complete_sessions": len(complete_sessions),
            "minimum_complete_sessions": 15,
            "complete_sessions_by_harness": {harness: complete_counts_by_harness[harness] for harness in ("claude", "codex")},
            "minimum_sessions_per_harness": 5,
            "all_sessions_complete": len(complete_sessions) == len(sessions),
        },
        "adjudication_status": adjudication_status,
        "independent_annotation_agreement": agreement,
        "user_spot_check": review_result,
        "conditions": dict(output_conditions),
        "paired_activity_macro_f1_improvement_vs_rules": paired_activity,
        "paired_prefix_vs_offline_activity_macro_f1": paired_context,
        "acceptance": acceptance,
    }


def _thermal_checkpoint() -> Callable[[], Any]:
    try:
        from real_pipeline import thermal_checkpoint
    except ImportError as exc:
        raise MetricsError("thermal checkpoint is unavailable") from exc
    return thermal_checkpoint


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    try:
        if path.stat().st_size > MAX_INPUT_BYTES:
            raise MetricsError("an input file exceeds the protocol size limit")
        rows: list[dict[str, Any]] = []
        with path.open("r", encoding="utf-8") as source:
            for line in source:
                if not line.strip():
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as exc:
                    raise MetricsError("an input JSONL file is invalid") from exc
                if not isinstance(row, dict):
                    raise MetricsError("input JSONL rows must be objects")
                rows.append(row)
        return rows
    except OSError as exc:
        raise MetricsError("an input file could not be read") from exc


def _read_json(path: Path) -> dict[str, Any]:
    try:
        if path.stat().st_size > MAX_INPUT_BYTES:
            raise MetricsError("an input file exceeds the protocol size limit")
        with path.open("r", encoding="utf-8") as source:
            value = json.load(source)
    except (OSError, json.JSONDecodeError) as exc:
        raise MetricsError("an input summary file is invalid") from exc
    if not isinstance(value, dict):
        raise MetricsError("summary input must be an object")
    return value


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--references", type=Path, required=True, help="Private reference JSONL; keep it local")
    parser.add_argument("--predictions", type=Path, action="append", required=True, help="Private benchmark predictions JSONL; repeat per run")
    parser.add_argument("--summaries", type=Path, action="append", default=[], help="Optional private benchmark summary JSON; repeat per run")
    parser.add_argument("--operations", type=Path, help="Disclosure-safe numeric aggregate from real_operations.py")
    parser.add_argument("--annotation-a", type=Path, help="Optional first independent annotation JSONL")
    parser.add_argument("--annotation-b", type=Path, help="Optional second independent annotation JSONL")
    parser.add_argument("--user-review", type=Path, help="Optional local spot-check review JSONL")
    parser.add_argument("--adjudication-status", choices=("pending", "frozen"), default="pending")
    parser.add_argument("--output", type=Path, required=True, help="Aggregate JSON output path outside private/")
    parser.add_argument("--draws", type=int, default=DEFAULT_DRAWS)
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    inputs = [args.references, *args.predictions, *args.summaries]
    if args.operations is not None:
        inputs.append(args.operations)
    inputs.extend(path for path in (args.annotation_a, args.annotation_b, args.user_review) if path is not None)
    output_path = args.output.resolve(strict=False)
    if any(path.resolve(strict=False) == output_path for path in inputs):
        parser.error("output must not overwrite an input")
    if (args.annotation_a is None) != (args.annotation_b is None):
        parser.error("both independent annotation files are required")
    try:
        checkpoint = _thermal_checkpoint()
        checkpoint()
        references = _read_jsonl(args.references)
        checkpoint()
        predictions: list[dict[str, Any]] = []
        for path in args.predictions:
            checkpoint()
            predictions.extend(_read_jsonl(path))
            checkpoint()
        summaries = []
        for path in args.summaries:
            checkpoint()
            summaries.append(_read_json(path))
            checkpoint()
        operations = _read_json(args.operations) if args.operations else None
        if args.operations:
            checkpoint()
        checkpoint()
        annotation_a = _read_jsonl(args.annotation_a) if args.annotation_a else None
        if args.annotation_a:
            checkpoint()
        if args.annotation_b:
            checkpoint()
        annotation_b = _read_jsonl(args.annotation_b) if args.annotation_b else None
        if args.annotation_b:
            checkpoint()
        if args.user_review:
            checkpoint()
        user_review = _read_jsonl(args.user_review) if args.user_review else None
        if args.user_review:
            checkpoint()
        checkpoint()
        result = score_records(
            references, predictions, summary_rows=summaries,
            operations=operations,
            annotation_a_rows=annotation_a, annotation_b_rows=annotation_b,
            user_review_rows=user_review, adjudication_status=args.adjudication_status,
            draws=args.draws, seed=args.seed, checkpoint=checkpoint,
        )
        checkpoint()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    except MetricsError as exc:
        parser.error(str(exc))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
