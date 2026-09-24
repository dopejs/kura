// Gateway mode is chosen at build time (VITE_KURA_GATEWAY_MODE=1) for the
// hosted build served by kura-gateway: the API is same-origin, and the
// gateway, not the browser, holds the daemon access token.
export function isGatewayMode(env: Record<string, unknown> = import.meta.env): boolean {
  return env.VITE_KURA_GATEWAY_MODE === "1";
}
