import { fetchToken } from "@/lib/api";
import { notFound } from "next/navigation";
import Link from "next/link";
import type { Token } from "@/types/token";

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-start py-3 border-b border-[#1e1e2e] last:border-0">
      <span className="w-40 text-sm text-slate-500 shrink-0">{label}</span>
      <span className="text-sm text-slate-200 break-all">{value}</span>
    </div>
  );
}


function fmtPrice(price: number | null) {
  if (price === null) return "—";
  if (price < 0.0001) return `$${price.toExponential(4)}`;
  return `$${price.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 8 })}`;
}

function fmtDate(iso: string | null) {
  if (!iso) return "—";
  return new Date(iso).toLocaleString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    timeZoneName: "short",
  });
}

export default async function TokenDetailPage({
  params,
}: {
  params: { address: string };
}) {
  const token = await fetchToken(params.address);
  if (!token) notFound();

  const title = token.symbol ?? token.token_address.slice(0, 10) + "…";
  return (
    <div className="max-w-2xl">
      {/* Back */}
      <Link
        href="/tokens"
        className="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-300 mb-6 transition-colors"
      >
        ← Back to Tokens
      </Link>

      {/* Header */}
      <div className="mb-8">
        <div className="flex items-center gap-3 mb-1">
          <h1 className="text-2xl font-bold text-slate-100">{title}</h1>
          {token.name && token.symbol && (
            <span className="text-slate-500">— {token.name}</span>
          )}
        </div>
        {token.is_fot && (
          <div className="mt-2">
            <span className="px-2 py-0.5 rounded text-xs font-semibold bg-amber-500/20 text-amber-400 uppercase tracking-wide">
              Transfer Fee
            </span>
          </div>
        )}
      </div>

      {/* Token info */}
      <div className="rounded-xl border border-[#1e1e2e] bg-[#111118] px-6 mb-4">
        <Row
          label="Address"
          value={
            <span className="font-mono text-xs text-indigo-300">
              {token.token_address}
            </span>
          }
        />
        <Row label="Symbol" value={token.symbol ?? "—"} />
        <Row label="Name" value={token.name ?? "—"} />
        <Row label="Decimals" value={token.decimals ?? "—"} />
        <Row label="Price" value={fmtPrice(token.price_usd)} />
        <Row
          label="Pool Count"
          value={token.pool_count.toLocaleString()}
        />
        <Row
          label="Last Updated"
          value={fmtDate(token.updated_at)}
        />
      </div>

      {/* Screening panel */}
      <div className="rounded-xl border border-[#1e1e2e] bg-[#111118] px-6">
        <div className="py-4 border-b border-[#1e1e2e]">
          <h2 className="text-sm font-semibold text-slate-300 uppercase tracking-wider">
            Screening Status
          </h2>
        </div>
        <Row
          label="Transfer Fee"
          value={
            token.is_fot ? (
              <span className="inline-flex items-center gap-2">
                <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium bg-red-500/15 text-red-400">
                  <span className="w-1.5 h-1.5 rounded-full bg-red-400" />
                  Fee-on-Transfer
                </span>
                {token.transfer_fee_bps != null && token.transfer_fee_bps > 0 && (
                  <span className="text-sm text-red-400 font-mono">
                    {(token.transfer_fee_bps / 100).toFixed(2)}%
                  </span>
                )}
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium bg-emerald-500/15 text-emerald-400">
                <span className="w-1.5 h-1.5 rounded-full bg-emerald-400" />
                Clean
              </span>
            )
          }
        />
        <Row
          label="Screened At"
          value={
            token.screened_at ? (
              fmtDate(token.screened_at)
            ) : (
              <span className="text-slate-500 italic">Not screened yet</span>
            )
          }
        />
      </div>
    </div>
  );
}
