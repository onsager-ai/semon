# Measured results

Weak agent annotations; repetitions are not independent corpus samples. Scores are recomputed against the current corrected reference, after verifying each saved native input hash. Original measured summaries and predictions remain unchanged. Offline-reference scores use the same reference for comparability; fixture excerpts are not complete native streams.

| Strategy | Schedule | View | Activity accuracy | Failure accuracy | Retry accuracy | Anomaly accuracy | Phase accuracy | HTTP P50/P95 ms | Events/s | Requests | Est. USD |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| rules | per-event | offline | 0.603 | 0.914 | 0.897 | 0.948 | 0.586 | n/a | 12785.78 | 0 | 0.000000 |
| rules | per-event | prefix | 0.603 | 0.914 | 0.897 | 0.948 | 0.534 | n/a | 12338.57 | 0 | 0.000000 |
| jev | event-triggered | prefix | 0.678 | 0.914 | 0.931 | 0.805 | 0.569 | 500.4/617.6 | 4.09 | 84 | 0.005608 |
| jev | microbatch | prefix | 0.580 | 0.885 | 0.943 | 0.851 | 0.448 | 503.2/682.2 | 3.48 | 93 | 0.014382 |
| jev | per-event | prefix | 0.563 | 0.885 | 0.914 | 0.759 | 0.420 | 495.9/556.9 | 2.01 | 174 | 0.011645 |
| llm | per-event | prefix | 0.718 | 0.960 | 0.983 | 0.925 | 0.529 | 4180.8/6644.1 | 0.23 | 174 | unknown |
| jev | per-event | offline | 0.534 | 0.862 | 0.862 | 0.782 | 0.437 | 505.5/569.5 | 1.97 | 174 | 0.012515 |
| llm | per-event | offline | 0.770 | 0.960 | 0.966 | 0.914 | 0.701 | 3998.7/6255.3 | 0.24 | 174 | unknown |

See comparison.json for precision, recall, macro F1, confusion matrices, strata, phase transition metrics, repeated-label stability, and paired offline comparisons. See failure-cases.jsonl for every disagreement with the weak reference.
