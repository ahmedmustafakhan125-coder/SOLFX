import { describe, expect, it } from "vitest";

import {
  CLUSTER_HINT,
  FAUCET_OFFER_BELOW,
  explorerTx,
  groupThousands,
  looksLikeWrongCluster,
  parseFaucetResponse,
  requestFaucet,
  shouldOfferFaucet,
  withClusterHint,
} from "@/lib/faucet";

const SIG =
  "5UfDuX7hXbDBZpHnSEFMwBN6JdANTF54fGVz9Kp1fZBNTmRmEiGPxk2NyDnA7b1vZkS4mFqh5rEPq9Kx3ZcYjQDv";

describe("when to offer the faucet", () => {
  it("offers it below 100 test USDC, not at or above", () => {
    expect(FAUCET_OFFER_BELOW).toBe(100_000_000n);
    expect(shouldOfferFaucet(0n)).toBe(true);
    expect(shouldOfferFaucet(99_999_999n)).toBe(true);
    expect(shouldOfferFaucet(100_000_000n)).toBe(false);
    expect(shouldOfferFaucet(10_000_000_000n)).toBe(false);
  });

  it("does not offer it before the balance is known", () => {
    // A button that flashes up while the balance loads, then vanishes, is worse than none.
    expect(shouldOfferFaucet(undefined)).toBe(false);
  });
});

describe("reading the faucet's answer", () => {
  it("a confirmed claim", () => {
    expect(
      parseFaucetResponse(200, { signature: SIG, usdc: "10000", sol: "0.05" })
    ).toEqual({
      ok: true,
      signature: SIG,
      usdc: "10000",
      sol: "0.05",
      pending: false,
    });
  });

  it("a claim sent but not yet confirmed is still a success, marked pending", () => {
    const r = parseFaucetResponse(202, {
      signature: SIG,
      usdc: "10000",
      sol: "0",
      pending: true,
    });
    expect(r.ok && r.pending).toBe(true);
  });

  it("shows the server's sentence for a refusal, including the time to wait", () => {
    const msg =
      "This wallet has already claimed test USDC in the last 24 hours. Try again in 23h 59m.";
    expect(
      parseFaucetResponse(429, { error: msg, retryAfterSeconds: 86_340 })
    ).toEqual({
      ok: false,
      error: msg,
    });
    expect(parseFaucetResponse(503, { error: "The faucet is empty." })).toEqual(
      { ok: false, error: "The faucet is empty." }
    );
  });

  it("falls back to a plain message when the body is not the faucet's", () => {
    // The site's own token bucket answers 429 with `{ error: "rate limited" }`; a proxy error
    // page may have no JSON at all.
    expect(parseFaucetResponse(502, undefined)).toEqual({
      ok: false,
      error: "The faucet did not answer (HTTP 502). Please try again.",
    });
    expect(parseFaucetResponse(429, {})).toEqual({
      ok: false,
      error: "Too many requests. Try again in a minute.",
    });
    // A 200 without a signature is not a success.
    expect(parseFaucetResponse(200, { usdc: "10000" }).ok).toBe(false);
  });
});

describe("requestFaucet", () => {
  it("posts the wallet as JSON to /faucet", async () => {
    let seen: { url: string; init: RequestInit | undefined } | undefined;
    const fetcher = (async (url: string, init?: RequestInit) => {
      seen = { url, init };
      return new Response(
        JSON.stringify({ signature: SIG, usdc: "10000", sol: "0.05" }),
        { status: 200 }
      );
    }) as unknown as typeof fetch;
    const r = await requestFaucet("WALLET", fetcher);
    expect(r.ok).toBe(true);
    expect(seen?.url).toBe("/faucet");
    expect(seen?.init?.method).toBe("POST");
    expect(JSON.parse(String(seen?.init?.body))).toEqual({ wallet: "WALLET" });
  });

  it("a network failure is a readable error, not a thrown one", async () => {
    const fetcher = (async () => {
      throw new TypeError("Failed to fetch");
    }) as unknown as typeof fetch;
    expect(await requestFaucet("WALLET", fetcher)).toEqual({
      ok: false,
      error: "Could not reach the faucet. Check your connection.",
    });
  });

  it("a non-JSON body still yields a message", async () => {
    const fetcher = (async () =>
      new Response("<html>bad gateway</html>", {
        status: 502,
      })) as unknown as typeof fetch;
    const r = await requestFaucet("WALLET", fetcher);
    expect(r).toEqual({
      ok: false,
      error: "The faucet did not answer (HTTP 502). Please try again.",
    });
  });
});

describe("the wrong-cluster hint", () => {
  it("is added to the errors a wallet on the wrong cluster, or with no SOL, produces", () => {
    for (const m of [
      "Transaction simulation failed: Blockhash not found",
      "Attempt to debit an account but found no record of a prior credit.",
      "AccountNotFound",
      "The account does not exist",
    ]) {
      expect(looksLikeWrongCluster(m)).toBe(true);
      expect(withClusterHint(m)).toBe(`${m} ${CLUSTER_HINT}`);
    }
  });

  it("is not added to a program's own refusal", () => {
    for (const m of [
      "SlippageExceeded: price moved beyond your limit",
      "LeverageTooHigh",
      "User rejected the request.",
    ]) {
      expect(withClusterHint(m)).toBe(m);
    }
  });

  it("is added once, however often the message passes through", () => {
    const once = withClusterHint("Blockhash not found");
    expect(withClusterHint(once)).toBe(once);
  });
});

describe("display helpers", () => {
  it("groups thousands without touching the decimals", () => {
    expect(groupThousands("10000")).toBe("10,000");
    expect(groupThousands("10000000")).toBe("10,000,000");
    expect(groupThousands("999")).toBe("999");
    expect(groupThousands("1234.5")).toBe("1,234.5");
  });

  it("links to the devnet explorer", () => {
    expect(explorerTx(SIG)).toBe(
      `https://explorer.solana.com/tx/${SIG}?cluster=devnet`
    );
  });
});
