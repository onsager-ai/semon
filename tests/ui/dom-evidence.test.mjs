import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compareDomEvidence, recordDomDiffSummary } from './dom-evidence.mjs';

test('summarizes bounded text and geometry differences across exact states', () => {
  const candidate = {
    selectors: {
      '#page': [{ text: 'Candidate answer', box: [8, 20, 280, 70], color: 'rgb(1, 2, 3)' }],
    },
  };
  const reference = {
    selectors: {
      '#page': [{ text: 'Reference answer', box: [8, 20, 280, 64], color: 'rgb(1, 2, 3)' }],
    },
  };
  const summary = { casesCompared: 0, casesWithChanges: 0, bySelector: {} };
  const differences = compareDomEvidence(candidate, reference, 'phone-light|session-harbor');
  recordDomDiffSummary(summary, differences);
  assert.equal(summary.casesCompared, 1);
  assert.equal(summary.casesWithChanges, 1);
  assert.equal(summary.bySelector['#page'].changedNodes, 1);
  assert.equal(summary.bySelector['#page'].changedFields.text, 1);
  assert.equal(summary.bySelector['#page'].changedFields.box, 1);
  assert.equal(summary.bySelector['#page'].examples[0].key, 'phone-light|session-harbor');
  assert.equal(
    summary.bySelector['#page'].examples[0].examples[0].candidateText,
    'Candidate answer',
  );
});

test('summaries cap state examples while preserving aggregate counts', () => {
  const summary = { casesCompared: 0, casesWithChanges: 0, bySelector: {} };
  for (let index = 0; index < 12; index++) {
    recordDomDiffSummary(
      summary,
      compareDomEvidence(
        { selector: '.region', nodes: [{ text: 'candidate', box: [0, index, 20, 20] }] },
        { selector: '.region', nodes: [{ text: 'reference', box: [0, index, 20, 20] }] },
        `frame-${index}`,
      ),
    );
  }
  assert.equal(summary.casesCompared, 12);
  assert.equal(summary.casesWithChanges, 12);
  assert.equal(summary.bySelector['.region'].changedNodes, 12);
  assert.equal(summary.bySelector['.region'].examples.length, 5);
});
