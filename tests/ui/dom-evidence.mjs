const FIELDS = ['text', 'box', 'font', 'color', 'background', 'padding', 'gap', 'style'];
const MAX_CASE_EXAMPLES = 3;
const MAX_SUMMARY_EXAMPLES = 5;

function nodesFor(evidence, selector) {
  if (Array.isArray(evidence?.selectors?.[selector])) return evidence.selectors[selector];
  if (evidence?.selector === selector && Array.isArray(evidence.nodes)) return evidence.nodes;
  return [];
}

function same(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function text(value) {
  return String(value ?? '')
    .replace(/\s+/g, ' ')
    .slice(0, 180);
}

function example(index, left, right, fields) {
  return {
    index,
    fields,
    candidateText: text(left?.text),
    referenceText: text(right?.text),
    candidateBox: left?.box ?? null,
    referenceBox: right?.box ?? null,
    candidateStyle: Object.fromEntries(
      ['font', 'color', 'background', 'padding', 'gap', 'style']
        .filter((field) => left?.[field] !== undefined)
        .map((field) => [field, left[field]]),
    ),
    referenceStyle: Object.fromEntries(
      ['font', 'color', 'background', 'padding', 'gap', 'style']
        .filter((field) => right?.[field] !== undefined)
        .map((field) => [field, right[field]]),
    ),
  };
}

export function compareDomEvidence(candidate, reference, key) {
  const selectors = new Set([
    ...Object.keys(candidate?.selectors ?? {}),
    ...Object.keys(reference?.selectors ?? {}),
  ]);
  if (candidate?.selector) selectors.add(candidate.selector);
  if (reference?.selector) selectors.add(reference.selector);
  return [...selectors].map((selector) => {
    const left = nodesFor(candidate, selector),
      right = nodesFor(reference, selector),
      changedFields = Object.fromEntries(FIELDS.map((field) => [field, 0])),
      examples = [];
    let changedNodes = 0;
    for (let index = 0; index < Math.max(left.length, right.length); index++) {
      const a = left[index],
        b = right[index],
        fields = a && b ? FIELDS.filter((field) => !same(a[field], b[field])) : ['presence'];
      if (!fields.length) continue;
      changedNodes++;
      for (const field of fields) if (Object.hasOwn(changedFields, field)) changedFields[field]++;
      if (examples.length < MAX_CASE_EXAMPLES) examples.push(example(index, a, b, fields));
    }
    return {
      key,
      selector,
      candidateCount: left.length,
      referenceCount: right.length,
      changedNodes,
      changedFields,
      examples,
    };
  });
}

export function recordDomDiffSummary(summary, comparisons) {
  summary.casesCompared++;
  if (comparisons.some((row) => row.changedNodes || row.candidateCount !== row.referenceCount))
    summary.casesWithChanges++;
  for (const row of comparisons) {
    const aggregate = (summary.bySelector[row.selector] ??= {
      casesCompared: 0,
      casesWithChanges: 0,
      changedNodes: 0,
      changedFields: Object.fromEntries(FIELDS.map((field) => [field, 0])),
      examples: [],
    });
    aggregate.casesCompared++;
    aggregate.changedNodes += row.changedNodes;
    if (row.changedNodes || row.candidateCount !== row.referenceCount) {
      aggregate.casesWithChanges++;
      if (aggregate.examples.length < MAX_SUMMARY_EXAMPLES)
        aggregate.examples.push({
          key: row.key,
          candidateCount: row.candidateCount,
          referenceCount: row.referenceCount,
          changedNodes: row.changedNodes,
          changedFields: row.changedFields,
          examples: row.examples,
        });
    }
    for (const field of FIELDS) aggregate.changedFields[field] += row.changedFields[field];
  }
}
