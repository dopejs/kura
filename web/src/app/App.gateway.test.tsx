import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { isGatewayMode } from "./gatewayMode";

const createdClientOptions: Array<{ baseURL: string; accessToken?: string }> = [];
const getMe = vi.fn().mockRejectedValue(new Error("gateway smoke"));

vi.mock("./gatewayMode", () => ({ isGatewayMode: () => true }));

vi.mock("@kura/client", () => ({
  createKuraClient: (options: { baseURL: string; accessToken?: string }) => {
    createdClientOptions.push(options);
    return new Proxy({ getMe }, {
      get: (target, prop) => (prop in target ? target[prop as keyof typeof target] : vi.fn().mockResolvedValue({ items: [] }))
    });
  }
}));

describe("App in gateway mode", () => {
  afterEach(() => cleanup());

  it("loads same-origin without a token and hides connection inputs", async () => {
    render(<App />);

    await waitFor(() => expect(getMe).toHaveBeenCalled());
    expect(createdClientOptions[0]).toMatchObject({ baseURL: window.location.origin, accessToken: undefined });
    expect(screen.queryByPlaceholderText("Bearer token")).toBeNull();
    expect(screen.queryByText("Daemon URL")).toBeNull();
    expect(screen.getByRole("link", { name: "Model settings" }).getAttribute("href")).toBe("/gw/settings");
    expect(screen.queryByText("Access token is required to load the operator shell.")).toBeNull();
  });
});

describe("isGatewayMode", () => {
  it("is opt-in", async () => {
    const actual = await vi.importActual<{ isGatewayMode: typeof isGatewayMode }>("./gatewayMode");
    expect(actual.isGatewayMode({})).toBe(false);
    expect(actual.isGatewayMode({ VITE_KURA_GATEWAY_MODE: "true" })).toBe(false);
    expect(actual.isGatewayMode({ VITE_KURA_GATEWAY_MODE: "1" })).toBe(true);
  });
});
