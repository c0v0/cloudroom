# Frontend component/hook test plan (first batch)

## Context

The frontend currently has only pure-logic Vitest tests (`src/lib/*.test.ts`); no React component or hook coverage. The goal is to choose the first 3–5 high-value targets whose behavior can be tested without real network, media devices, or heavy third-party mocks. This plan adds the minimum test dependencies/configuration and five focused tests; it does not chase coverage or test implementation details.

Execution also writes this plan to `docs/plans/2026-10-21-frontend-component-tests.md`.

## Inventory and value × testability

| Target | User-visible value | Dependencies / testability | Verdict |
|---|---|---|---|
| `App.tsx` + `auth.tsx` route/nav | 权限/角色显示、route guards | Router/Auth context; lazy pages can be mocked; no network | **First batch** |
| `Register.tsx` invite/bootstrap flow | 邀请码 auto-fill, URL hygiene, form validation | Router/API/auth; mock API seam | **First batch** |
| `Settings.tsx` password/preferences | 设置表单校验、localStorage prefs | Large modal; auth/API + noise WASM import; mock 2 seams | **First batch** |
| `useRoomSocket.ts` | WS connect/event/reconnect/cleanup | Only global WebSocket; fake timers; no real network | **First batch** |
| `useVoiceSession.ts` | P2P/SFU adapter selection, join state | Two adapter hooks; module mocks; no media devices | **First batch (integration seam)** |
| `Home.tsx` room request form | request validation/guest upgrade | API/router; testable, lower risk than above | Next batch |
| `Admin.tsx` tabs/invites/users | invite revoke, role/user admin UI | Large component, many API calls, internal tabs not exported | Next batch |
| `Room.tsx` message list/voice controls | message list, join UI, settings/invites | 1,625 lines + WS/media/router; no isolated presentational component | Defer until extraction |
| `useMicTest` / `useP2PVoice` / `useLiveKitVoice` | media capture/WebRTC quality | MediaDevices/AudioContext/third-party internals; high mock cost | Defer; covered by runtime UI smoke instead |
| `denoise` / `volumeMeter` / `remoteAudio` / `chime` / `theme` / `throttle` | settings/audio utilities | Mostly pure but lower user-visible risk; existing pure tests cover some | Later, targeted unit tests only |
| `lib/messages`, `roles`, `roomCode`, `time` | shared logic | Already tested | Done |

Message list note: the merge logic lives in the tested `lib/messages.ts`; the UI list is embedded in `Room.tsx`. Do not build a `Room` test with a dozen mocks. If message-list UI deserves tests, first extract a small presentational `MessageList` component (separate change), then test it with props.

## Dependencies and Vitest setup

Added dev dependencies:

```bash
cd frontend
npm install --save-dev \
  @testing-library/react@^16.3.0 \
  @testing-library/dom@^10.4.0 \
  @testing-library/user-event@^14.6.1 \
  @testing-library/jest-dom@^6.9.1
```

Updated `frontend/vitest.config.ts`:

```ts
export default defineConfig({
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["./src/test/setup.ts"],
    restoreMocks: true,
  },
});
```

Created `frontend/src/test/setup.ts` with jest-dom, RTL cleanup, localStorage cleanup, fake-timer reset, and a matchMedia polyfill.

No MSW, no jsdom network, no WebRTC polyfill. Component tests mock the narrow seams (`./auth`, `../api`, voice adapters); hook tests use a fake WebSocket or adapter mocks.

## First-batch targets

### 1. `src/App.test.tsx` — role navigation and route guard

Behavior tested:

- Loading auth state shows `加载中…` and no navigation.
- A normal user sees no `管理后台` link; admin sees it.
- `/admin` redirects a normal user to home; admin renders the admin route.
- Auth failures/absence do not render admin content.

Method: RTL + `MemoryRouter`; `vi.hoisted` mutable auth object + `vi.mock("./auth")`; tiny mocks for Home/Admin/Room/Settings; no API calls.

### 2. `src/pages/Register.test.tsx` — invite token and password validation

Behavior tested:

- `?invite=<token>` pre-fills the invite field and removes the token from the URL.
- Password mismatch shows `两次输入的密码不一致` and does not call the API.
- Missing invite shows `请输入邀请码...` and does not call the API.
- Valid input calls `POST /auth/register` with the captured token and navigates home.

Method: RTL + `user-event` + `MemoryRouter`; mock auth/API; no real fetch.

### 3. `src/pages/Settings.test.tsx` — password form validation

Behavior tested:

- Mismatch shows `两次输入的新密码不一致` and does not call `api`.
- Matching passwords call `/api/v1/me/password`, clear fields, show `密码已修改。`.
- API rejection shows the error and leaves the form usable.

Method: RTL + `user-event`; mock auth/API and denoise module (avoid loading rnnoise WASM); real localStorage/theme/dynamics helpers are safe in jsdom.

### 4. `src/hooks/useRoomSocket.test.ts` — connect/event/reconnect/cleanup

Behavior tested:

- Mount creates one WebSocket; `onopen` sets `connected=true`.
- `hello`/`voice_state` update `voiceMembers` and invoke the event callback.
- Server close schedules a 1000ms reconnect and creates a second socket.
- Unmount closes the socket and suppresses reconnects.

Method: `renderHook` + fake timers + a small fake global WebSocket; no real network.

### 5. `src/hooks/useVoiceSession.test.tsx` — mode selects the correct adapter

Behavior tested:

- `mode="p2p"` enables P2P, disables SFU, exposes P2P status/inVoice/participant count/join.
- Rerender with `mode="sfu"` flips selection and exposes SFU state.
- `handleSignal` comes from P2P.
- Switching modes does not auto-call either adapter's `join`.

Method: `renderHook` with module mocks for `useP2PVoice`/`useLiveKitVoice`; no media internals mocked.

## Explicit exclusions

- No UI snapshot tests.
- No third-party internal tests (`livekit-client`, rnnoise WASM, media engines).
- No coverage-threshold padding.
- No real network/WebSocket/microphone/camera/audio devices.
- No `Room.tsx`/`useMicTest`/adapter-internal tests in the first batch.

## Acceptance criteria

- App: user role removes admin link and `/admin` redirects; admin sees/renders it.
- Register: invite query consumed, mismatch blocks API, valid submit includes token.
- Settings: mismatch blocks API; valid request clears fields and shows success.
- `useRoomSocket`: unmount stops reconnect; close before unmount reconnects; events reach callback.
- `useVoiceSession`: p2p/sfu enable exactly one adapter; mode switch does not auto-join.
- Tests deterministic, no network/device, no CSS/DOM snapshots.

## Verification

From `/home/hui/agents/omp/cloudroom/frontend`:

```bash
npm test
npm run typecheck
npm run build
```

From `/home/hui/agents/omp/cloudroom`:

```bash
scripts/check.sh
```

Expected: existing 24 pure-logic tests plus new tests pass; typecheck/build green; check.sh retains 16 PASS / 0 FAIL / 2 SKIP (frontend test stage includes new tests).

## Assumptions & contingencies

- First batch is five targets; message-list UI and Admin invite tabs deferred.
- Component tests mock auth/API at module seams rather than running a server.
- If `Settings` still pulls rnnoise WASM, keep the denoise module mock; do not add a global WASM loader.
- If `@testing-library/react@16` resolves another `@testing-library/dom` peer, keep the explicit v10 install; do not use legacyRoot.
- No production component changes are required; tests target default exports/hooks.

## Rollback

Revert the test/config/dependency commit. Remove added tests/setup, restore Vitest config and package files; no production code or runtime behavior changes.
