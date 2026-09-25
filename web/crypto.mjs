// Artifact encryption, run entirely in the client.
//
// A protected artifact is encrypted before upload; the hub stores only the
// envelope and the ciphertext. The envelope names the algorithm, the key
// derivation, the iteration count, and the salts, so any client with the
// password can decrypt without server help.
//
// This module is shared by the PWA and by the offline self-test the build gate
// runs, so it must stay free of any Node-only or browser-only API.

const ITERATIONS = 600000;
const MIN_ITERATIONS = 100000;
const MAX_ITERATIONS = 10000000;
const SALT_BYTES = 16;
const IV_BYTES = 12;

// Set as `code` on the refusals a password cannot fix: the envelope names an
// algorithm or an iteration count this module will not accept. A wrong
// password and tampered data stay unmarked, and stay indistinguishable from
// each other, because the cipher reports both the same way.
export const UNSUPPORTED_ENVELOPE = "unsupported-envelope";

// Set as `code` when the browser has no Web Crypto at all. It is not a wrong
// password: `crypto.subtle` is only exposed in a secure context, so a hub
// opened over plain LAN HTTP cannot decrypt anything a password would fix.
export const INSECURE_CONTEXT = "insecure-context";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function bytesToBase64(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function base64ToBytes(text) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

// Bind the ciphertext to the parameters it was sealed with, so an altered
// envelope fails authentication rather than silently deriving a wrong key. The
// string is built field by field because the hub stores the envelope as JSON
// and may not preserve key order.
function envelopeAad(envelope) {
  return [
    `alg=${envelope.alg}`,
    `kdf=${envelope.kdf}`,
    `iterations=${envelope.iterations}`,
    `salt=${envelope.salt}`,
    `iv=${envelope.iv}`,
  ].join(";");
}

function unsupported(message) {
  const error = new Error(message);
  error.code = UNSUPPORTED_ENVELOPE;
  return error;
}

function iterationsOf(envelope) {
  const count = Number(envelope.iterations);
  if (!Number.isInteger(count) || count < MIN_ITERATIONS || count > MAX_ITERATIONS) {
    throw unsupported("unsupported iteration count");
  }
  return count;
}

async function deriveKey(password, salt, iterations) {
  // Web Crypto is only exposed in a secure context. Plain LAN HTTP, which the
  // quickstart documents, has no `crypto.subtle`, and the failure otherwise
  // reads as a wrong password. H17.
  if (!globalThis.crypto || !globalThis.crypto.subtle) {
    const refused = new Error(
      "Web Crypto is unavailable: this page is not a secure context",
    );
    refused.code = INSECURE_CONTEXT;
    throw refused;
  }
  const material = await crypto.subtle.importKey(
    "raw",
    encoder.encode(password),
    "PBKDF2",
    false,
    ["deriveKey"],
  );
  return crypto.subtle.deriveKey(
    { name: "PBKDF2", salt, iterations, hash: "SHA-256" },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
}

// Encrypt plaintext with a password. Returns the envelope and a base64
// ciphertext, both ready to store as text.
export async function encrypt(password, plaintext) {
  return seal(password, plaintext, ITERATIONS);
}

// The iteration count is a parameter only so the self-test can seal a valid
// envelope below the floor and prove decrypt refuses it.
async function seal(password, plaintext, iterations) {
  if (!password) throw new Error("a password is required");
  const salt = crypto.getRandomValues(new Uint8Array(SALT_BYTES));
  const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
  const envelope = {
    alg: "AES-256-GCM",
    kdf: "PBKDF2-HMAC-SHA256",
    iterations,
    salt: bytesToBase64(salt),
    iv: bytesToBase64(iv),
  };
  const key = await deriveKey(password, salt, iterations);
  const sealed = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv, additionalData: encoder.encode(envelopeAad(envelope)) },
    key,
    encoder.encode(plaintext),
  );
  return { envelope, ciphertext: bytesToBase64(new Uint8Array(sealed)) };
}

// Decrypt a base64 ciphertext with the envelope and password. Throws when the
// password is wrong, the envelope was altered, or it is not one this module
// produced; only the last carries `code === UNSUPPORTED_ENVELOPE`.
export async function decrypt(password, envelope, ciphertext) {
  if (!password) throw new Error("a password is required");
  if (!envelope || envelope.alg !== "AES-256-GCM") {
    throw unsupported("unsupported encryption envelope");
  }
  const iterations = iterationsOf(envelope);
  const salt = base64ToBytes(envelope.salt);
  const iv = base64ToBytes(envelope.iv);
  const key = await deriveKey(password, salt, iterations);
  const opened = await crypto.subtle.decrypt(
    {
      name: "AES-GCM",
      iv,
      additionalData: encoder.encode(envelopeAad({ ...envelope, iterations })),
    },
    key,
    base64ToBytes(ciphertext),
  );
  return decoder.decode(opened);
}

// A known-plaintext round-trip, run by `make web/crypto`.
export async function selfTest() {
  const plaintext = "the hub holds no plaintext, only this ciphertext";
  const first = await encrypt("correct horse", plaintext);
  const second = await encrypt("correct horse", plaintext);
  if (first.envelope.iterations !== ITERATIONS || first.envelope.alg !== "AES-256-GCM") {
    throw new Error("the envelope does not describe the expected algorithm");
  }
  if (first.ciphertext === second.ciphertext || first.envelope.iv === second.envelope.iv) {
    throw new Error("two encryptions reused a nonce");
  }
  const opened = await decrypt("correct horse", first.envelope, first.ciphertext);
  if (opened !== plaintext) {
    throw new Error("the round-trip did not recover the plaintext");
  }
  const altered = { ...first.envelope, iv: bytesToBase64(new Uint8Array(IV_BYTES)) };
  let tamperRejected = false;
  try {
    await decrypt("correct horse", altered, first.ciphertext);
  } catch {
    tamperRejected = true;
  }
  if (!tamperRejected) throw new Error("an altered envelope was accepted");
  let wrongRejected = false;
  let wrongCode;
  try {
    await decrypt("wrong horse", first.envelope, first.ciphertext);
  } catch (error) {
    wrongRejected = true;
    wrongCode = error.code;
  }
  if (!wrongRejected) throw new Error("a wrong password decrypted the ciphertext");
  if (wrongCode === UNSUPPORTED_ENVELOPE) {
    throw new Error("a wrong password was marked as an unsupported envelope");
  }
  // Sealed for real below the floor, so only the floor check can refuse it.
  const weak = await seal("correct horse", plaintext, MIN_ITERATIONS - 1);
  let weakRejected = false;
  let weakCode;
  try {
    await decrypt("correct horse", weak.envelope, weak.ciphertext);
  } catch (error) {
    weakRejected = true;
    weakCode = error.code;
  }
  if (!weakRejected) throw new Error("an envelope below the iteration floor was accepted");
  if (weakCode !== UNSUPPORTED_ENVELOPE) {
    throw new Error("a refused envelope did not carry the unsupported marker");
  }
  let foreignCode;
  try {
    await decrypt("correct horse", { ...first.envelope, alg: "AES-128-CBC" }, first.ciphertext);
  } catch (error) {
    foreignCode = error.code;
  }
  if (foreignCode !== UNSUPPORTED_ENVELOPE) {
    throw new Error("a foreign algorithm did not carry the unsupported marker");
  }
  const flipped = base64ToBytes(first.ciphertext);
  flipped[0] ^= 0xff;
  let ciphertextRejected = false;
  try {
    await decrypt("correct horse", first.envelope, bytesToBase64(flipped));
  } catch {
    ciphertextRejected = true;
  }
  if (!ciphertextRejected) throw new Error("a tampered ciphertext byte decrypted");
  // `kdf` feeds neither the key nor the cipher, so only the binding catches it.
  const relabelled = { ...first.envelope, kdf: "PBKDF2-HMAC-SHA512" };
  let aadRejected = false;
  try {
    await decrypt("correct horse", relabelled, first.ciphertext);
  } catch {
    aadRejected = true;
  }
  if (!aadRejected) throw new Error("a tampered authenticated field decrypted");
  console.log("web/crypto: round-trip passed");
}
