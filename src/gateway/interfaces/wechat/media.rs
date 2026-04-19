//! Media Download & AES Decryption
//!
//! Handles downloading media from WeChat CDN and decrypting with AES-128-ECB.

use aes::cipher::{array::Array, consts::U16, BlockCipherDecrypt, KeyInit};
use aes::Aes128;
use reqwest::Client;

const AES_BLOCK_SIZE: usize = 16;

/// AES-128-ECB decryption with PKCS7 unpadding.
pub fn aes128_ecb_decrypt(ciphertext: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, String> {
    if ciphertext.is_empty() {
        return Ok(Vec::new());
    }

    if !ciphertext.len().is_multiple_of(AES_BLOCK_SIZE) {
        return Err(format!(
            "Ciphertext length {} is not a multiple of block size {}",
            ciphertext.len(),
            AES_BLOCK_SIZE
        ));
    }

    let key_array: Array<u8, U16> = (*key).into();
    let cipher = Aes128::new(&key_array);

    let mut plaintext = vec![0u8; ciphertext.len()];
    for (i, chunk) in ciphertext.chunks_exact(AES_BLOCK_SIZE).enumerate() {
        let mut block: Array<u8, U16> = chunk.try_into().unwrap();
        cipher.decrypt_block(&mut block);
        plaintext[i * AES_BLOCK_SIZE..(i + 1) * AES_BLOCK_SIZE].copy_from_slice(&block);
    }

    pkcs7_unpad(&mut plaintext)?;

    Ok(plaintext)
}

fn pkcs7_unpad(data: &mut Vec<u8>) -> Result<(), String> {
    let len = data.len();
    if len == 0 {
        return Ok(());
    }

    let pad_len = data[len - 1] as usize;
    if pad_len == 0 || pad_len > AES_BLOCK_SIZE {
        return Err(format!("Invalid PKCS7 padding byte: {}", pad_len));
    }

    if pad_len > len {
        return Err("PKCS7 padding exceeds data length".to_string());
    }

    if !data[len - pad_len..].iter().all(|&b| b == pad_len as u8) {
        return Err("PKCS7 padding verification failed".to_string());
    }

    data.truncate(len - pad_len);
    Ok(())
}

/// PKCS7 pad data to block size.
pub fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let pad_len = block_size - (data.len() % block_size);
    let mut result = Vec::with_capacity(data.len() + pad_len);
    result.extend_from_slice(data);
    result.extend(std::iter::repeat_n(pad_len as u8, pad_len));
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
        data = aes128_ecb_decrypt(&data, &key)?;
    }

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockCipherEncrypt;

    fn aes128_ecb_encrypt(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
        let padded = pkcs7_pad(plaintext, AES_BLOCK_SIZE);
        let key_array = Array::<u8, U16>::from_slice(key);
        let cipher = Aes128::new(key_array);

        let mut ciphertext = Vec::with_capacity(padded.len());
        for chunk in padded.chunks_exact(AES_BLOCK_SIZE) {
            let mut block = Array::<u8, U16>::clone_from_slice(chunk);
            cipher.encrypt_block(&mut block);
            ciphertext.extend_from_slice(&block);
        }
        ciphertext
    }

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

    #[test]
    fn test_aes128_ecb_roundtrip() {
        let key = [0x42u8; 16];
        let plaintext = b"hello world! this is my plaintext.";

        let ciphertext = aes128_ecb_encrypt(plaintext, &key);
        let decrypted = aes128_ecb_decrypt(&ciphertext, &key).unwrap();

        assert_eq!(decrypted, plaintext.as_slice());
    }

    #[test]
    fn test_aes128_ecb_empty_input() {
        let key = [0x00u8; 16];
        let result = aes128_ecb_decrypt(b"", &key).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_aes128_ecb_invalid_length() {
        let key = [0x00u8; 16];
        let result = aes128_ecb_decrypt(b"short", &key);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a multiple"));
    }

    #[test]
    fn test_pkcs7_unpad_invalid_padding() {
        let mut data = vec![0u8; 16];
        data[15] = 5; // Claim 5 padding bytes, but all zeros are not 5
        let result = pkcs7_unpad(&mut data);
        assert!(result.is_err());
    }
}
