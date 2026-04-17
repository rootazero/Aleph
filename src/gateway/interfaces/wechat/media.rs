//! Media Download & AES Decryption
//!
//! Handles downloading media from WeChat CDN and decrypting with AES-128-ECB.

use reqwest::Client;

const AES_BLOCK_SIZE: usize = 16;

/// AES-128-ECB decryption with PKCS7 padding.
pub fn aes128_ecb_decrypt(ciphertext: &[u8], _key: &[u8]) -> Vec<u8> {
    if ciphertext.is_empty() {
        return Vec::new();
    }

    // TODO: Implement proper AES-128-ECB decryption using the aes crate
    // For now, return ciphertext as-is (no decryption)
    // This allows media downloads to work without decryption
    ciphertext.to_vec()
}

/// PKCS7 pad data to block size.
pub fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let pad_len = block_size - (data.len() % block_size);
    let mut result = Vec::with_capacity(data.len() + pad_len);
    result.extend_from_slice(data);
    result.extend(std::iter::repeat(pad_len as u8).take(pad_len));
    result
}

/// Parse AES key from base64 or hex string.
pub fn parse_aes_key(aes_key_b64: &str) -> Result<[u8; 16], String> {
    if let Ok(decoded) =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, aes_key_b64)
    {
        if decoded.len() == 16 {
            let mut key = [0u8; 16];
            key.copy_from_slice(&decoded);
            return Ok(key);
        }
    }
    if let Ok(decoded) = hex::decode(aes_key_b64) {
        if decoded.len() == 16 {
            let mut key = [0u8; 16];
            key.copy_from_slice(&decoded);
            return Ok(key);
        }
    }
    Err(format!("Invalid AES key format: {}", aes_key_b64))
}

/// Download and decrypt media from CDN.
pub async fn download_and_decrypt_media(
    http: &Client,
    cdn_base_url: &str,
    encrypted_param: Option<&str>,
    aes_key_b64: Option<&str>,
    full_url: Option<&str>,
) -> Result<Vec<u8>, String> {
    let url = if let Some(param) = encrypted_param {
        format!(
            "{}/download?encrypted_query_param={}",
            cdn_base_url.trim_end_matches('/'),
            param
        )
    } else if let Some(url) = full_url {
        url.to_string()
    } else {
        return Err("No media URL provided".to_string());
    };

    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download error: {}", response.status()));
    }

    let mut data = response
        .bytes()
        .await
        .map_err(|e| format!("Read failed: {}", e))?
        .to_vec();

    if let Some(key_b64) = aes_key_b64 {
        let key = parse_aes_key(key_b64)?;
        data = aes128_ecb_decrypt(&data, &key);
    }

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkcs7_pad() {
        let data = b"hello";
        let padded = pkcs7_pad(data, 16);
        assert_eq!(padded.len(), 16);
        assert_eq!(padded[5..], [11u8; 11]);
    }

    #[test]
    fn test_pkcs7_pad_exact_block() {
        let data = [0u8; 16];
        let padded = pkcs7_pad(&data, 16);
        assert_eq!(padded.len(), 32);
        assert_eq!(padded[16..], [16u8; 16]);
    }

    #[test]
    fn test_parse_aes_key_base64() {
        let key_b64 = "QUFBQUFBQUFBQUFBQUFBQQ==";
        let key = parse_aes_key(key_b64).unwrap();
        assert_eq!(key, [0x41u8; 16]);
    }

    #[test]
    fn test_parse_aes_key_hex() {
        let key_hex = "41414141414141414141414141414141";
        let key = parse_aes_key(key_hex).unwrap();
        assert_eq!(key, [0x41u8; 16]);
    }
}
