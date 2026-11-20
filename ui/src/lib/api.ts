export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export interface JsonObject {
  [key: string]: Json;
}
export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly history?: { done: number; total: number; retryMs: number } | 'recovery_failed',
  ) {
    super(message);
  }
}
export async function requestJson(
  path: string,
  signal?: AbortSignal,
  unchanged = false,
  body?: unknown,
): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(path, {
      credentials: 'same-origin',
      signal,
      ...(body === undefined
        ? {}
        : {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
          }),
    });
  } catch (error) {
    if (error instanceof Error && error.name === 'AbortError') throw error;
    throw new ApiError(error instanceof Error ? error.message : 'No response', 0);
  }
  if (unchanged && response.status === 304) return null;
  if (!response.ok) {
    if (
      response.status === 503 &&
      response.headers.get('Content-Type')?.startsWith('application/json')
    ) {
      const payload: unknown = await response.json().catch(() => null);
      if (payload && typeof payload === 'object' && 'error' in payload) {
        const error = payload.error;
        if (error && typeof error === 'object' && 'code' in error) {
          if (error.code === 'history_recovery_failed')
            throw new ApiError('History recovery failed', 503, 'recovery_failed');
          if (error.code === 'history_rebuilding' && 'progress' in error) {
            const progress = error.progress;
            if (
              progress &&
              typeof progress === 'object' &&
              'done' in progress &&
              'total' in progress &&
              typeof progress.done === 'number' &&
              typeof progress.total === 'number' &&
              Number.isSafeInteger(progress.done) &&
              Number.isSafeInteger(progress.total) &&
              progress.done >= 0 &&
              progress.total >= progress.done
            ) {
              const seconds = Number(response.headers.get('Retry-After')) || 1;
              throw new ApiError('History rebuilding', 503, {
                done: progress.done,
                total: progress.total,
                retryMs: Math.min(8000, Math.max(1000, seconds * 1000)),
              });
            }
          }
        }
      }
    }
    // Other error readers need only the status. Release an unread body rather than
    // leaving its transport alive after the caller retires its deadline.
    try {
      await response.body?.cancel();
    } catch {}
    throw new ApiError(response.status + ' ' + response.statusText, response.status);
  }
  return response.json() as Promise<unknown>;
}
