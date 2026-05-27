import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Arb Pulse",
  description: "PulseChain DEX arbitrage monitor",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body className="min-h-screen bg-[#0a0a0f] text-slate-200">
        <nav className="border-b border-[#1e1e2e] px-6 py-4 flex items-center gap-8">
          <span className="text-indigo-400 font-bold tracking-tight text-lg">
            Arb Pulse
          </span>
          <a
            href="/tokens"
            className="text-sm text-slate-400 hover:text-slate-200 transition-colors"
          >
            Tokens
          </a>
        </nav>
        <main className="max-w-7xl mx-auto px-6 py-8">{children}</main>
      </body>
    </html>
  );
}
