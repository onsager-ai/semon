// Passive browser instrumentation for the synthetic long-session fixture. It observes
// existing fetches and their existing JSON reads: no cloned bodies or extra requests.
export function installRefreshTracing() {
  const now = () => performance.timeOrigin + performance.now();
  const describe = (input) => {
    const url = new URL(typeof input === 'string' ? input : input.url, location.href);
    if (!url.pathname.startsWith('/api/')) return null;
    const query = Object.fromEntries(
      ['machine', 'sid', 'after', 'limit', 'generation', 'errors_generation']
        .filter((key) => url.searchParams.has(key))
        .map((key) => [key, url.searchParams.get(key)]),
    );
    return { endpoint: url.pathname, query };
  };
  window.__perfFetchTrace = [];
  const nativeFetch = window.fetch;
  window.fetch = async function (input, init) {
    const description = describe(input);
    if (!description) return nativeFetch.call(this, input, init);
    const entry = { id: window.__perfFetchTrace.length, ...description, admittedAt: now() };
    window.__perfFetchTrace.push(entry);
    const signal = init?.signal ?? input?.signal;
    const aborted = () => {
      entry.abortedAt = now();
    };
    signal?.addEventListener('abort', aborted, { once: true });
    try {
      const response = await nativeFetch.call(this, input, init);
      entry.responseAt = now();
      entry.status = response.status;
      entry.ok = response.ok;
      // Wrap only the method the application already calls. In particular, do
      // not read error bodies that requestJson intentionally cancels unread.
      const nativeJson = response.json;
      response.json = async function () {
        entry.jsonStartedAt = now();
        try {
          const value = await nativeJson.call(this);
          entry.jsonCompletedAt = now();
          entry.generation = value?.projection?.generation ?? value?.generation ?? null;
          entry.total = value?.projection?.total ?? null;
          entry.selected =
            value?.items?.find((item) => item.key === 'marathon')?.transcript ?? null;
          entry.refresh = value?.refresh ?? null;
          return value;
        } catch (error) {
          entry.jsonFailedAt = now();
          entry.jsonError = error.name;
          throw error;
        } finally {
          signal?.removeEventListener('abort', aborted);
        }
      };
      if (!response.ok) signal?.removeEventListener('abort', aborted);
      return response;
    } catch (error) {
      entry.failedAt = now();
      entry.error = error.name;
      signal?.removeEventListener('abort', aborted);
      throw error;
    }
  };
}

export function armRefreshPaint(marker) {
  window.__perfRefreshPaint?.observer.disconnect();
  const state = {
    marker,
    armedAt: performance.timeOrigin + performance.now(),
    commitAt: null,
    visibleAt: null,
  };
  const observer = new MutationObserver(() => {
    const messages = document.querySelectorAll(
      '#page section[aria-label="Transcript"] .msg.assistant',
    );
    const last = messages[messages.length - 1];
    if (!last?.textContent.includes(marker) || state.commitAt !== null) return;
    state.commitAt = performance.timeOrigin + performance.now();
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (last.isConnected && last.textContent.includes(marker)) {
          state.visibleAt = performance.timeOrigin + performance.now();
          observer.disconnect();
        } else state.commitAt = null;
      }),
    );
  });
  observer.observe(document.querySelector('#page'), {
    subtree: true,
    childList: true,
    characterData: true,
  });
  window.__perfRefreshPaint = { state, observer };
}

export function readRefreshTrace(start) {
  const { state } = window.__perfRefreshPaint;
  const end = state.visibleAt ?? performance.timeOrigin + performance.now();
  return {
    ...state,
    // Retain JSON reads admitted before the append while their application read
    // is outstanding. Error headers already handed to requestJson are terminal
    // here; its unread-body cancellation has its own CDP transport terminal event.
    fetches: window.__perfFetchTrace.filter(
      (entry) =>
        entry.admittedAt <= end &&
        (entry.jsonCompletedAt ??
          entry.jsonFailedAt ??
          entry.failedAt ??
          (entry.ok === false ? entry.responseAt : Infinity)) >= start,
    ),
  };
}
