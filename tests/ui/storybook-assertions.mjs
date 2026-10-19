// A text filter is applied before cardinality. Default visible/hidden/focused
// assertions mean exactly one match; absence means zero matches. Plural states
// must identify every expected row in DOM order, then verify every row's state.
export function assertionErrors(assertion) {
  const errors = [];
  if (
    !assertion ||
    !['visible', 'absent', 'hidden', 'focused'].includes(assertion.kind) ||
    typeof assertion.selector !== 'string' ||
    !assertion.selector.trim()
  )
    return ['Invalid visible-state assertion'];
  const count = assertion.count ?? (assertion.kind === 'absent' ? 0 : 1);
  if (
    !Number.isSafeInteger(count) ||
    count < 0 ||
    count > 100 ||
    (assertion.kind === 'absent' ? count !== 0 : count === 0) ||
    (assertion.kind === 'focused' && count !== 1)
  )
    errors.push('Invalid assertion count');
  if (
    assertion.text !== undefined &&
    (typeof assertion.text !== 'string' || !assertion.text.trim())
  )
    errors.push('Invalid assertion text filter');
  if (assertion.kind === 'absent' && (assertion.text !== undefined || assertion.rows !== undefined))
    errors.push('Absence must cover the entire selector');
  if (count > 1 && !Array.isArray(assertion.rows))
    errors.push('Plural assertions require expected row identities and text');
  if (assertion.rows !== undefined) {
    if (!Array.isArray(assertion.rows) || assertion.rows.length !== count)
      errors.push('Expected row count differs from assertion count');
    else {
      const identities = new Set();
      for (const row of assertion.rows) {
        const entries =
          row?.attributes && typeof row.attributes === 'object' && !Array.isArray(row.attributes)
            ? Object.entries(row.attributes)
            : [];
        if (
          !entries.length ||
          entries.some(
            ([name, value]) =>
              !/^[a-zA-Z][\w:-]*$/.test(name) || typeof value !== 'string' || !value.trim(),
          ) ||
          typeof row.text !== 'string' ||
          !row.text.trim()
        )
          errors.push('Each expected row needs attributes and text');
        const identity = JSON.stringify(
          entries.sort(([left], [right]) => left.localeCompare(right)),
        );
        if (identities.has(identity)) errors.push('Duplicate expected row identity');
        identities.add(identity);
      }
    }
  }
  if (
    assertion.expectedWidths &&
    Object.entries(assertion.expectedWidths).some(
      ([width, value]) =>
        ![334, 390, 696, 820, 1280].includes(Number(width)) ||
        !Number.isFinite(value) ||
        value <= 0,
    )
  )
    errors.push('Invalid expected component width');
  if (
    assertion.scrollTop !== undefined &&
    (!Number.isFinite(assertion.scrollTop) || assertion.scrollTop < 0 || count !== 1)
  )
    errors.push('Invalid expected scroll position');
  return errors;
}

async function waitForCount(locator, expected, selector, timeoutMs, pollMs) {
  const deadline = Date.now() + timeoutMs;
  let actual;
  do {
    actual = await locator.count();
    if (actual === expected) return;
    if (Date.now() >= deadline) break;
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  } while (true);
  throw new Error(`Expected ${expected} matching elements for ${selector}, got ${actual}`);
}

export async function verifyAssertions(page, assertions, { timeoutMs = 10000, pollMs = 100 } = {}) {
  for (const assertion of assertions) {
    const errors = assertionErrors(assertion);
    if (errors.length) throw new Error(errors.join('; '));
    const locator = page.locator(assertion.selector);
    const targets = assertion.text ? locator.filter({ hasText: assertion.text }) : locator;
    const count = assertion.count ?? (assertion.kind === 'absent' ? 0 : 1);
    await waitForCount(targets, count, assertion.selector, timeoutMs, pollMs);
    for (let index = 0; index < count; index++) {
      const target = targets.nth(index);
      await target.waitFor({
        state: assertion.kind === 'hidden' ? 'hidden' : 'visible',
        timeout: timeoutMs,
      });
      const row = assertion.rows?.[index];
      if (row) {
        for (const [name, value] of Object.entries(row.attributes))
          if ((await target.getAttribute(name)) !== value)
            throw new Error(`${assertion.selector} row ${index}: expected ${name}=${value}`);
        const text =
          assertion.kind === 'hidden' ? await target.textContent() : await target.innerText();
        if (!text?.includes(row.text))
          throw new Error(`${assertion.selector} row ${index}: expected text ${row.text}`);
      }
      if (
        assertion.kind === 'focused' &&
        !(await target.evaluate((node) => node === document.activeElement))
      )
        throw new Error(`expected focused element ${assertion.selector}`);
      if (
        assertion.scrollTop !== undefined &&
        Math.abs((await target.evaluate((node) => node.scrollTop)) - assertion.scrollTop) > 0.5
      )
        throw new Error(`expected ${assertion.selector} scrollTop ${assertion.scrollTop}`);
      const expectedWidth = assertion.expectedWidths?.[page.viewportSize().width];
      if (expectedWidth !== undefined) {
        const box = await target.boundingBox();
        if (!box || Math.abs(box.width - expectedWidth) > 1)
          throw new Error(
            `expected ${assertion.selector} width ${expectedWidth}, got ${box?.width}`,
          );
      }
      if (assertion.inViewport) {
        const contained = await target.evaluate((node) => {
          const box = node.getBoundingClientRect();
          let left = 0,
            top = 0,
            right = innerWidth,
            bottom = innerHeight;
          for (let parent = node.parentElement; parent; parent = parent.parentElement) {
            const css = getComputedStyle(parent),
              clip = parent.getBoundingClientRect();
            if (/(auto|scroll|hidden|clip)/.test(css.overflowX)) {
              left = Math.max(left, clip.left);
              right = Math.min(right, clip.right);
            }
            if (/(auto|scroll|hidden|clip)/.test(css.overflowY)) {
              top = Math.max(top, clip.top);
              bottom = Math.min(bottom, clip.bottom);
            }
          }
          return (
            box.width > 0 &&
            box.height > 0 &&
            box.left >= left - 1 &&
            box.top >= top - 1 &&
            box.right <= right + 1 &&
            box.bottom <= bottom + 1
          );
        });
        if (!contained)
          throw new Error(
            `claimed visible ownership is outside capture viewport: ${assertion.selector}`,
          );
      }
    }
    // Catch changed cardinality during the visibility waits.
    if ((await targets.count()) !== count)
      throw new Error(`Assertion collection changed for ${assertion.selector}`);
  }
}
