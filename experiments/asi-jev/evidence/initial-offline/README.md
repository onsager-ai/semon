# Superseded offline representation

These 174 Jev requests used the first offline formatter, which moved TARGET to the end and did not preserve its position relative to future records. The original observations are retained as an experiment failure case. They are excluded from `results/` and the main comparison. Their input usage was 297,975 tokens, or $0.01251495 at TypeSafe's published list price; exact gateway billing is unknown.

The corresponding initial Haiku offline run was stopped with SIGINT when the format defect was identified. It did not finish or write result files. Its partial token use and cost are unknown and are excluded from completed-trial totals. Stopping the local client does not establish that the in-flight provider request was cancelled.

The corrected formatter renders TARGET in source order and is covered by a specific regression. Corrected offline runs use the run ID `ordered-20261029`. The streaming formatter and native source bytes did not change.
