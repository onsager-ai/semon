import { assertionErrors } from './storybook-assertions.mjs';

// Contract validation and browser capture resolve exactly the same state keys.
export function makeCapturePlan(contract, inventory, storyIds) {
  const errors = [], plan = new Map(), stories = new Set(storyIds);
  const dimensions = new Set([334, 390, 696, 820, 1280]);
  function matrix(item, label) {
    if (!Array.isArray(item.widths) || !item.widths.length || new Set(item.widths).size !== item.widths.length)
      errors.push(`${label}: explicit unique widths required`);
    if (!Array.isArray(item.schemes) || !item.schemes.length || new Set(item.schemes).size !== item.schemes.length)
      errors.push(`${label}: explicit unique schemes required`);
    for (const width of item.widths ?? []) if (!dimensions.has(width)) errors.push(`${label}: unsupported width ${width}`);
    for (const scheme of item.schemes ?? []) if (!contract.pixelPolicy.schemes.includes(scheme)) errors.push(`${label}: unsupported theme ${scheme}`);
  }
  function assertions(items, label, required = false) {
    if (!Array.isArray(items) || (required && !items.length)) {
      errors.push(`${label}: assertions array required`);
      return;
    }
    for (const assertion of items) errors.push(...assertionErrors(assertion).map((error) => `${label}: ${error}`));
  }
  function actions(items, label, storyId) {
    if (!Array.isArray(items) || !items.length) {
      errors.push(`${label}: executable actions[] required`);
      return;
    }
    for (const action of items) {
      if (!['click', 'tap', 'hover', 'focus', 'press', 'owner-unmount'].includes(action.kind) || !action.selector)
        errors.push(`${label}: unsupported action ${action.kind}`);
      if (action.kind === 'press' && !action.key) errors.push(`${label}: keyboard key required`);
      if (action.kind === 'owner-unmount' && (storyId !== 'semon-production-owners--gallery-lifecycle' || action.selector !== '[data-storybook-owner="gallery-lifecycle"]'))
        errors.push(`${label}: owner-unmount is restricted to its test-only lifecycle owner`);
      if (action.waitMs !== undefined && (!Number.isInteger(action.waitMs) || action.waitMs < 0 || action.waitMs > 5000)) errors.push(`${label}: invalid action waitMs`);
      assertions(action.assertionsAfter ?? [], label);
    }
  }
  function add(item) {
    const key = `${item.storyId}|${item.width}|${item.scheme}${item.interactionId ? `|${item.interactionId}` : ''}`;
    if (plan.has(key)) errors.push(`duplicate executable capture ${key}`);
    if (!stories.has(item.storyId)) errors.push(`unknown story ${item.storyId}`);
    plan.set(key, { ...item, key,
      expectedState: item.interactionId ?? inventory.stories?.[item.storyId]?.expectedState,
      referenceDisposition: 'pending-storybook-review',
    });
  }
  const defaults = { widths: contract.pixelPolicy.dimensions, schemes: contract.pixelPolicy.schemes };
  matrix(defaults, 'default capture matrix');
  for (const [storyId, state] of Object.entries(inventory.stories ?? {})) {
    if (!stories.has(storyId)) errors.push(`unknown story state ${storyId}`);
    if (!state.expectedState?.trim()) errors.push(`story ${storyId}: explicit expected state required`);
    assertions(state.assertions, `story ${storyId}`, true);
  }
  for (const storyId of stories) if (!inventory.stories?.[storyId]) errors.push(`missing explicit story state ${storyId}`);
  for (const storyId of stories) for (const width of defaults.widths) for (const scheme of defaults.schemes)
    add({ storyId, width, scheme, actions: [], assertions: [...(inventory.stories?.[storyId]?.assertions ?? [])], postCaptureActions: [], postCaptureAssertions: [] });
  const interactions = new Set();
  for (const item of contract.interactionCaptures ?? []) {
    const label = `interaction ${item.id}`;
    if (!item.id || interactions.has(item.id)) errors.push(`${label}: missing/duplicate ID`);
    interactions.add(item.id);
    if (Object.hasOwn(item, 'action') || Object.hasOwn(item, 'waitMs')) errors.push(`${label}: singular action and top-level waitMs are forbidden`);
    matrix(item, label);
    assertions(item.assertions, label, true);
    assertions(item.postCaptureAssertions ?? [], label);
    if (item.postCaptureActions?.length) actions(item.postCaptureActions, label, item.storyId);
    for (const width of Object.keys(item.widthActions ?? {})) if (!item.widths.includes(Number(width))) errors.push(`${label}: unused widthActions ${width}`);
    for (const width of item.widths ?? []) {
      const sequence = item.widthActions?.[width] ?? item.actions;
      actions(sequence, `${label} at ${width}`, item.storyId);
      for (const scheme of item.schemes ?? []) add({
        storyId: item.storyId, interactionId: item.id, width, scheme,
        actions: sequence, assertions: item.assertions,
        postCaptureActions: item.postCaptureActions ?? [], postCaptureAssertions: item.postCaptureAssertions ?? [],
      });
    }
  }
  for (const [id, target] of Object.entries(inventory.targets ?? {})) {
    if (!target.storyId) continue;
    matrix(target, `target ${id}`);
    assertions(target.assertions, `target ${id}`, true);
    for (const width of target.widths ?? []) for (const scheme of target.schemes ?? []) {
      const key = `${target.storyId}|${width}|${scheme}`;
      if (!plan.has(key)) add({ storyId: target.storyId, width, scheme, actions: [], assertions: [...(inventory.stories?.[target.storyId]?.assertions ?? [])], postCaptureActions: [], postCaptureAssertions: [] });
      plan.get(key).assertions.push(...target.assertions);
    }
  }
  if (errors.length) throw new Error(errors.join('\n'));
  return [...plan.values()];
}
