import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api";
import { CommandPalette } from "./CommandPalette";

vi.mock("../api", async (importOriginal) => {
  const mod = await importOriginal<typeof import("../api")>();
  return { ...mod, api: vi.fn() };
});

const apiMock = vi.mocked(api);

function Location() {
  const location = useLocation();
  return <div data-testid="location">{location.pathname + location.search}</div>;
}

describe("CommandPalette", () => {
  beforeEach(() => {
    apiMock.mockReset();
    apiMock.mockImplementation(async (path: string) => {
      if (path.startsWith("/api/v1/search")) {
        return {
          results: [
            {
              message_id: "m1",
              room_code: "alpha",
              seq: 3,
              body: "needle in history",
              created_at: "2026-11-05T00:00:00Z",
              sender_nickname: "Alice",
            },
          ],
        };
      }
      return { rooms: [{ code: "alpha", kind: "standard" }] };
    });
  });

  it("filters rooms and opens a message deep link", async () => {
    render(
      <MemoryRouter initialEntries={["/"]}>
        <CommandPalette open onClose={() => undefined} />
        <Location />
      </MemoryRouter>,
    );

    expect(await screen.findByText("alpha")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("全局搜索关键词"), "needle");
    expect(await screen.findByText("needle in history")).toBeInTheDocument();
    await userEvent.click(screen.getByText("needle in history"));
    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/alpha?msg=m1&seq=3",
      ),
    );
  });
});
