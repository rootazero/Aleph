//! Browser byte persistence. JS is statically bundled by wasm-bindgen (no eval).

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = r#"
const DB = 'aleph.tray.v1';
const STORE = 'items';
const MAX = 50 * 1024 * 1024;
function open() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    let abandoned = false;
    const fail = error => { abandoned = true; reject(error); };
    request.onblocked = () => fail(new Error('IndexedDB open blocked'));
    request.onerror = () => { request.onblocked = request.onerror = request.onupgradeneeded = request.onsuccess = null; fail(request.error); };
    request.onupgradeneeded = () => {
      if (abandoned) { request.transaction.abort(); return; }
      try { if (!request.result.objectStoreNames.contains(STORE)) request.result.createObjectStore(STORE); }
      catch (error) { try { request.transaction.abort(); } finally { fail(error); } }
    };
    request.onsuccess = () => {
      request.onblocked = request.onerror = request.onupgradeneeded = request.onsuccess = null;
      const db = request.result;
      if (abandoned) { db.close(); return; }
      db.onversionchange = () => db.close();
      resolve(db);
    };
  });
}
async function transact(op, key, bytes) {
  const db = await open();
  try {
    return await new Promise((resolve, reject) => {
      let tx, request, value, error;
      const clear = () => {
        if (request) request.onsuccess = request.onerror = null;
        if (tx) tx.oncomplete = tx.onabort = tx.onerror = null;
      };
      try {
        tx = db.transaction(STORE, op === 'get' ? 'readonly' : 'readwrite');
        tx.oncomplete = () => { clear(); if (error) reject(error); else resolve(value); };
        tx.onerror = () => { error = tx.error || error || new Error('IndexedDB transaction failed'); };
        tx.onabort = () => { const failure = tx.error || error || new Error('IndexedDB transaction aborted'); clear(); reject(failure); };
        const store = tx.objectStore(STORE);
        request = op === 'put' ? store.put(bytes, key) : op === 'get' ? store.get(key) : store.delete(key);
        request.onsuccess = () => { value = request.result; };
        request.onerror = () => { error = request.error; }; // default action aborts transaction
      } catch (failure) {
        error = failure;
        if (tx) { try { tx.abort(); } catch (_) { clear(); reject(failure); } }
        else { clear(); reject(failure); }
      }
    });
  } finally { db.close(); }
}
export async function trayPut(key, bytes) {
  if (bytes.byteLength > MAX) throw new Error('Tray item exceeds 50 MiB');
  await transact('put', key, bytes);
  return bytes.byteLength;
}
export async function trayPutBase64(key, value) {
  const binary = atob(value);
  const bytes = Uint8Array.from(binary, ch => ch.charCodeAt(0));
  return trayPut(key, bytes);
}
export async function trayFetch(key, url) {
  const response = await fetch(url, {credentials: 'same-origin'});
  if (!response.ok) throw new Error('Artifact fetch failed: HTTP ' + response.status);
  // Stop over-limit bodies as they arrive; Content-Length is not trusted.
  const reader = response.body?.getReader();
  if (!reader) return trayPut(key, new Uint8Array(await response.arrayBuffer()));
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      const {done, value} = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX) { await reader.cancel(); throw new Error('Tray item exceeds 50 MiB'); }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return trayPut(key, bytes);
}
export async function trayGet(key) {
  const value = await transact('get', key);
  if (value == null) throw new Error('Pinned bytes are missing from IndexedDB');
  return new Uint8Array(value);
}
export async function trayDelete(key) { await transact('delete', key); }
export function trayEncode(bytes) {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 8192)
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return btoa(binary);
}
export function trayDownload(bytes, name, mime) {
  const url = URL.createObjectURL(new Blob([bytes], {type: mime}));
  const anchor = document.createElement('a');
  anchor.href = url; anchor.download = name;
  document.body.append(anchor);
  try { anchor.click(); } finally { anchor.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000); }
}
export function attachmentRead(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onloadend = () => {
      reader.onloadend = reader.onabort = null;
      if (reader.error) reject(reader.error);
      else if (typeof reader.result === 'string') resolve(reader.result.split(',')[1]);
      else reject(new Error('File read returned no data'));
    };
    reader.onabort = () => { reader.onabort = reader.onloadend = null; reject(new Error('File read aborted')); };
    try { reader.readAsDataURL(file); } catch (error) { reader.onabort = reader.onloadend = null; reject(error); }
  });
}
"#)]
extern "C" {
    #[wasm_bindgen(catch, js_name = trayPutBase64)]
    async fn put_base64(key: &str, value: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = trayFetch)]
    async fn fetch(key: &str, url: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = trayGet)]
    async fn get(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = trayDelete)]
    async fn delete(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = trayEncode)]
    fn encode(bytes: &[u8]) -> Result<String, JsValue>;
    #[wasm_bindgen(catch, js_name = trayDownload)]
    fn download_bytes(bytes: &[u8], name: &str, mime: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = attachmentRead)]
    async fn read_file(file: &web_sys::File) -> Result<JsValue, JsValue>;
}

pub(super) async fn persist_bytes_base64(key: &str, value: &str) -> Result<usize, String> {
    #[cfg(target_arch = "wasm32")]
    {
        put_base64(key, value)
            .await
            .map(|v| v.as_f64().unwrap_or_default() as usize)
            .map_err(|e| format!("{e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, value);
        Err("IndexedDB unavailable on native host".into())
    }
}
pub(super) async fn fetch_into_idb(url: &str, key: &str) -> Result<usize, String> {
    #[cfg(target_arch = "wasm32")]
    {
        fetch(key, url)
            .await
            .map(|v| v.as_f64().unwrap_or_default() as usize)
            .map_err(|e| format!("{e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (url, key);
        Err("IndexedDB unavailable on native host".into())
    }
}
pub(super) async fn delete_bytes(key: &str) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        delete(key).await.map(|_| ()).map_err(|e| format!("{e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        Err("IndexedDB unavailable on native host".into())
    }
}
pub(super) async fn read_bytes(key: &str) -> Result<Vec<u8>, String> {
    #[cfg(target_arch = "wasm32")]
    {
        get(key)
            .await
            .map(|v| js_sys::Uint8Array::new(&v).to_vec())
            .map_err(|e| format!("{e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        Err("IndexedDB unavailable on native host".into())
    }
}
pub(crate) async fn read_attachment(file: &web_sys::File) -> Result<String, String> {
    #[cfg(target_arch = "wasm32")]
    {
        read_file(file)
            .await
            .map_err(|e| format!("{e:?}"))?
            .as_string()
            .ok_or_else(|| "File read returned no data".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = file;
        Err("FileReader unavailable on native host".into())
    }
}
pub(super) fn bytes_to_base64(bytes: &[u8]) -> Result<String, String> {
    #[cfg(target_arch = "wasm32")]
    {
        encode(bytes).map_err(|e| format!("{e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = bytes;
        Err("Browser encoding unavailable on native host".into())
    }
}
pub(super) fn download(bytes: &[u8], name: &str, mime: &str) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        download_bytes(bytes, name, mime).map_err(|e| format!("{e:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (bytes, name, mime);
        Err("Browser downloads unavailable on native host".into())
    }
}
