"use client";

import { useEffect, useState, useCallback } from "react";
import { useRouter } from "next/navigation";
import { fetchTokens } from "@/lib/api";
import type { Token, TokensResponse } from "@/types/token";

const PAGE_SIZE = 50;

function shortAddr(addr: string) {
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`;
}

function fmtPrice(price: number | null) {
  if (price === null) return "—";
  if (price < 0.0001) return `$${price.toExponential(2)}`;
  return `$${price.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 6 })}`;
}

function Badge({ label, color }: { label: string; color: string }) {
  return (
    <span
      className={`inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-semibold uppercase tracking-wide ${color}`}
    >
      {label}
    </span>
  );
}

function TokenRow({ token, onClick }: { token: Token; onClick: () => void }) {
  return (
    <tr
      onClick={onClick}
      className="border-b border-[#1e1e2e] hover:bg-[#111118] cursor-pointer transition-colors"
    >
      <td className="px-4 py-3">
        <div className="flex items-center gap-2">
          <span className="font-medium text-slate-100">
            {token.symbol ?? <span className="text-slate-500 italic">unknown</span>}
          </span>
          {token.is_fot && (
            <Badge label="Transfer Fee" color="bg-amber-500/20 text-amber-400" />
          )}
        </div>
      </td>
      <td className="px-4 py-3 text-slate-400 text-sm">
        {token.name ?? "—"}
      </td>
      <td className="px-4 py-3 font-mono text-xs text-slate-500">
        {shortAddr(token.token_address)}
      </td>
      <td className="px-4 py-3 text-slate-400 text-sm text-right">
        {token.decimals ?? "—"}
      </td>
      <td className="px-4 py-3 text-slate-300 text-sm text-right">
        {fmtPrice(token.price_usd)}
      </td>
      <td className="px-4 py-3 text-slate-400 text-sm text-right">
        {token.pool_count.toLocaleString()}
      </td>
      <td className="px-4 py-3 text-center">
        {token.screened_at ? (
          <span className="text-emerald-400 text-xs">✓</span>
        ) : (
          <span className="text-slate-600 text-xs">—</span>
        )}
      </td>
    </tr>
  );
}

export default function TokensPage() {
  const router = useRouter();
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [page, setPage] = useState(0);
  const [data, setData] = useState<TokensResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Debounce search input
  useEffect(() => {
    const t = setTimeout(() => {
      setDebouncedSearch(search);
      setPage(0);
    }, 300);
    return () => clearTimeout(t);
  }, [search]);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await fetchTokens({
        q: debouncedSearch,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      });
      setData(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load tokens");
    } finally {
      setLoading(false);
    }
  }, [debouncedSearch, page]);

  useEffect(() => {
    load();
  }, [load]);

  const totalPages = data ? Math.ceil(data.total / PAGE_SIZE) : 0;

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-bold text-slate-100">Tokens</h1>
          {data && (
            <p className="text-sm text-slate-500 mt-1">
              {data.total.toLocaleString()} tokens in database
            </p>
          )}
        </div>
      </div>

      {/* Search */}
      <div className="mb-4">
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search by address, symbol, or name…"
          className="w-full max-w-md bg-[#111118] border border-[#1e1e2e] rounded-lg px-4 py-2.5 text-sm text-slate-200 placeholder-slate-600 focus:outline-none focus:border-indigo-500 transition-colors"
        />
      </div>

      {/* Table */}
      <div className="rounded-xl border border-[#1e1e2e] overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="bg-[#111118] border-b border-[#1e1e2e]">
              <th className="px-4 py-3 text-left text-xs font-medium text-slate-500 uppercase tracking-wider">
                Symbol
              </th>
              <th className="px-4 py-3 text-left text-xs font-medium text-slate-500 uppercase tracking-wider">
                Name
              </th>
              <th className="px-4 py-3 text-left text-xs font-medium text-slate-500 uppercase tracking-wider">
                Address
              </th>
              <th className="px-4 py-3 text-right text-xs font-medium text-slate-500 uppercase tracking-wider">
                Decimals
              </th>
              <th className="px-4 py-3 text-right text-xs font-medium text-slate-500 uppercase tracking-wider">
                Price
              </th>
              <th className="px-4 py-3 text-right text-xs font-medium text-slate-500 uppercase tracking-wider">
                Pools
              </th>
              <th className="px-4 py-3 text-center text-xs font-medium text-slate-500 uppercase tracking-wider">
                Screened
              </th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <tr>
                <td colSpan={7} className="px-4 py-12 text-center text-slate-500">
                  Loading…
                </td>
              </tr>
            ) : error ? (
              <tr>
                <td colSpan={7} className="px-4 py-12 text-center text-red-400">
                  {error}
                </td>
              </tr>
            ) : data?.tokens.length === 0 ? (
              <tr>
                <td colSpan={7} className="px-4 py-12 text-center text-slate-500">
                  No tokens found
                </td>
              </tr>
            ) : (
              data?.tokens.map((token) => (
                <TokenRow
                  key={token.token_address}
                  token={token}
                  onClick={() => router.push(`/tokens/${token.token_address}`)}
                />
              ))
            )}
          </tbody>
        </table>
      </div>

      {/* Pagination */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between mt-4">
          <span className="text-sm text-slate-500">
            Page {page + 1} of {totalPages}
          </span>
          <div className="flex gap-2">
            <button
              disabled={page === 0}
              onClick={() => setPage((p) => p - 1)}
              className="px-3 py-1.5 rounded-lg text-sm bg-[#111118] border border-[#1e1e2e] text-slate-400 hover:text-slate-200 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              Previous
            </button>
            <button
              disabled={page >= totalPages - 1}
              onClick={() => setPage((p) => p + 1)}
              className="px-3 py-1.5 rounded-lg text-sm bg-[#111118] border border-[#1e1e2e] text-slate-400 hover:text-slate-200 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              Next
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
