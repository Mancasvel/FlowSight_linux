import assert from "node:assert/strict";
import test from "node:test";
import {
  decryptTokenBundle,
  encryptTokenBundle,
  exchangeAuthorizationCode,
  NotionApiClient,
} from "./notion_api.ts";

test("OAuth token bundle is encrypted at rest and round-trips", async () => {
  const rawKey = crypto.getRandomValues(new Uint8Array(32));
  let binary = "";
  for (const byte of rawKey) binary += String.fromCharCode(byte);
  const key = btoa(binary);
  const accessToken = crypto.randomUUID();
  const refreshCredential = crypto.randomUUID();
  const encrypted = await encryptTokenBundle({
    access_token: accessToken,
    refresh_token: refreshCredential,
  }, key);
  assert.equal(encrypted.version, 1);
  assert.ok(!encrypted.ciphertext.includes(accessToken));
  assert.ok(!encrypted.ciphertext.includes(refreshCredential));
  const decrypted = await decryptTokenBundle({
    token_ciphertext: encrypted.ciphertext,
    token_iv: encrypted.iv,
    encryption_version: encrypted.version,
  }, key);
  assert.equal(decrypted.access_token, accessToken);
  assert.equal(decrypted.refresh_token, refreshCredential);
});

test("Notion and OAuth failures never echo credentials", async () => {
  const credential = crypto.randomUUID();
  const failingFetch = async () => new Response(JSON.stringify({
    code: "unauthorized",
    message: `rejected ${credential}`,
  }), { status: 401, headers: { "Content-Type": "application/json" } });

  await assert.rejects(
    () => new NotionApiClient(credential, failingFetch as typeof fetch).search("page"),
    (error: Error) => {
      assert.doesNotMatch(error.message, new RegExp(credential));
      assert.match(error.message, /unauthorized/);
      return true;
    },
  );
  await assert.rejects(
    () => exchangeAuthorizationCode({
      code: credential,
      clientId: crypto.randomUUID(),
      clientSecret: credential,
      redirectUri: "https://example.invalid/callback",
      fetcher: failingFetch as typeof fetch,
    }),
    (error: Error) => {
      assert.equal(error.message, "Notion OAuth exchange failed.");
      assert.doesNotMatch(error.message, new RegExp(credential));
      return true;
    },
  );
});
