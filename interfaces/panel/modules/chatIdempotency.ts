/**
 * A random version-4 UUID for `idempotency_key`. Not `crypto.randomUUID()`:
 * the desktop shell's embedded runtime has no Web Crypto (enabling it pulls
 * in RSA and elliptic-curve crates). The key only has to be unique within
 * this caller's namespace while the Gateway caches it, not unguessable.
 */
export function newIdempotencyKey(): string {
  const hex = (digits: number) =>
    Array.from({ length: digits }, () => Math.floor(Math.random() * 16).toString(16)).join("");
  const variant = (8 + Math.floor(Math.random() * 4)).toString(16);
  return `${hex(8)}-${hex(4)}-4${hex(3)}-${variant}${hex(3)}-${hex(12)}`;
}
