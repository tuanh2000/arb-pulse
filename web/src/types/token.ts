export interface Token {
  token_address: string;
  symbol: string | null;
  name: string | null;
  decimals: number | null;
  is_fot: boolean;
  is_meme: boolean;
  transfer_fee_bps: number | null;
  screened_at: string | null;
  price_usd: number | null;
  pool_count: number;
  updated_at: string;
}

export interface TokensResponse {
  tokens: Token[];
  total: number;
  limit: number;
  offset: number;
}
