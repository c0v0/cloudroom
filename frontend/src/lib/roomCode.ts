const ROOM_CODE_PATH = /^[a-z0-9-]{3,32}$/;

/** Match the route-segment rule used by the SPA (`/:code`). */
export function isRoomCodePath(code: string): boolean {
  return ROOM_CODE_PATH.test(code);
}

/** Normalize user input the same way the backend normalizes custom codes. */
export function normalizeRoomCode(input: string): string {
  return input.trim().toLowerCase();
}
