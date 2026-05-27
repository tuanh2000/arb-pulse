import type { Token, TokensResponse } from "@/types/token";

const API_URL = process.env.NEXT_PUBLIC_API_URL ?? "http://localhost:4000";

export async function fetchTokens(params: {
  q?: string;
  limit?: number;
  offset?: number;
}): Promise<TokensResponse> {
  const url = new URL(`${API_URL}/api/tokens`);
  if (params.q) url.searchParams.set("q", params.q);
  if (params.limit) url.searchParams.set("limit", String(params.limit));
  if (params.offset) url.searchParams.set("offset", String(params.offset));

  const res = await fetch(url.toString(), { cache: "no-store" });
  if (!res.ok) throw new Error(`Failed to fetch tokens: ${res.status}`);
  return res.json();
}

export async function fetchToken(address: string): Promise<Token | null> {
  const res = await fetch(`${API_URL}/api/tokens/${address}`, {
    cache: "no-store",
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`Failed to fetch token: ${res.status}`);
  return res.json();
}
