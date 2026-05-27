import type { Config } from "tailwindcss";

const config: Config = {
  content: ["./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      fontFamily: { mono: ["var(--font-mono)", "monospace"] },
    },
  },
  plugins: [],
};
export default config;
