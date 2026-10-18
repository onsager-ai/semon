export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export interface JsonObject {
  [key: string]: Json;
}
export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
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
    // Error readers need only the status. Release an unread body rather than
    // leaving its transport alive after the caller retires its deadline.
    try {
      await response.body?.cancel();
    } catch {}
    throw new ApiError(response.status + ' ' + response.statusText, response.status);
  }
  return response.json() as Promise<unknown>;
}
