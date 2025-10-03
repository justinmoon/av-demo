use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes128Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use hkdf::Hkdf;
use sha2::Sha256;
use std::collections::HashMap;

#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

/// Media encryption key manager
///
/// Per MOQ_MARMOT_AV_SPEC.md:
/// - base = MLS-Exporter("moq-media-base-v1", sender_leaf || track_label || epoch_bytes, 32)
/// - K_gen, N_salt = HKDF(base, "k"/"n" || gen)
/// - Generation = MSB of 32-bit frame counter; cache ~10s
pub struct MediaCrypto {
    base_key: [u8; 32],
    key_cache: HashMap<u8, CachedGeneration>,
    #[cfg(not(target_arch = "wasm32"))]
    cache_ttl: Duration,
}

struct CachedGeneration {
    aead_key: [u8; 16],
    nonce_salt: [u8; 12],
    #[cfg(not(target_arch = "wasm32"))]
    created_at: Instant,
}

impl MediaCrypto {
    /// Create new MediaCrypto with base key from MLS exporter
    pub fn new(base_key: [u8; 32]) -> Self {
        Self {
            base_key,
            key_cache: HashMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            cache_ttl: Duration::from_secs(10),
        }
    }

    /// Derive AEAD key and nonce salt for a given generation using HKDF
    fn derive_generation_keys(&self, generation: u8) -> Result<([u8; 16], [u8; 12])> {
        let hkdf = Hkdf::<Sha256>::new(None, &self.base_key);

        // Derive AEAD key: HKDF(base, "k" || gen)
        let mut aead_key = [0u8; 16];
        let k_info = [b'k', generation];
        hkdf.expand(&k_info, &mut aead_key)
            .map_err(|_| anyhow!("HKDF expand failed for key"))?;

        // Derive nonce salt: HKDF(base, "n" || gen)
        let mut nonce_salt = [0u8; 12];
        let n_info = [b'n', generation];
        hkdf.expand(&n_info, &mut nonce_salt)
            .map_err(|_| anyhow!("HKDF expand failed for nonce"))?;

        Ok((aead_key, nonce_salt))
    }

    /// Get or derive keys for a generation, with caching
    fn get_generation_keys(&mut self, generation: u8) -> Result<(&[u8; 16], &[u8; 12])> {
        // Evict expired cache entries (not available in WASM)
        #[cfg(not(target_arch = "wasm32"))]
        {
            let now = Instant::now();
            self.key_cache
                .retain(|_, cached| now.duration_since(cached.created_at) < self.cache_ttl);
        }

        // Get or create cache entry
        if !self.key_cache.contains_key(&generation) {
            let (aead_key, nonce_salt) = self.derive_generation_keys(generation)?;
            self.key_cache.insert(
                generation,
                CachedGeneration {
                    aead_key,
                    nonce_salt,
                    #[cfg(not(target_arch = "wasm32"))]
                    created_at: Instant::now(),
                },
            );
        }

        let cached = self.key_cache.get(&generation).unwrap();
        Ok((&cached.aead_key, &cached.nonce_salt))
    }

    /// Construct nonce from frame counter and nonce salt
    /// Frame counter is 32-bit, MSB becomes generation
    fn construct_nonce(nonce_salt: &[u8; 12], frame_counter: u32) -> [u8; 12] {
        let mut nonce = *nonce_salt;
        // XOR the last 4 bytes with frame counter (big-endian)
        for (i, byte) in frame_counter.to_be_bytes().iter().enumerate() {
            nonce[8 + i] ^= byte;
        }
        nonce
    }

    /// Encrypt media frame
    ///
    /// # Arguments
    /// * `plaintext` - Raw media payload to encrypt
    /// * `frame_counter` - 32-bit frame counter (MSB = generation)
    /// * `aad` - Additional authenticated data (track label, epoch, etc.)
    pub fn encrypt(&mut self, plaintext: &[u8], frame_counter: u32, aad: &[u8]) -> Result<Vec<u8>> {
        // Extract generation from MSB of frame counter
        let generation = (frame_counter >> 24) as u8;

        let (aead_key, nonce_salt) = self.get_generation_keys(generation)?;
        let nonce_bytes = Self::construct_nonce(nonce_salt, frame_counter);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let cipher = Aes128Gcm::new(aead_key.into());
        let payload = Payload {
            msg: plaintext,
            aad,
        };

        cipher
            .encrypt(nonce, payload)
            .map_err(|e| anyhow!("AEAD encryption failed: {e}"))
    }

    /// Decrypt media frame
    ///
    /// # Arguments
    /// * `ciphertext` - Encrypted media payload
    /// * `frame_counter` - 32-bit frame counter (MSB = generation)
    /// * `aad` - Additional authenticated data (must match encryption)
    pub fn decrypt(
        &mut self,
        ciphertext: &[u8],
        frame_counter: u32,
        aad: &[u8],
    ) -> Result<Vec<u8>> {
        // Extract generation from MSB of frame counter
        let generation = (frame_counter >> 24) as u8;

        let (aead_key, nonce_salt) = self.get_generation_keys(generation)?;
        let nonce_bytes = Self::construct_nonce(nonce_salt, frame_counter);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let cipher = Aes128Gcm::new(aead_key.into());
        let payload = Payload {
            msg: ciphertext,
            aad,
        };

        cipher
            .decrypt(nonce, payload)
            .map_err(|e| anyhow!("AEAD decryption failed: {e}"))
    }
}

/// Additional Authenticated Data builder
///
/// Per spec: AAD binds version, group root, track label, epoch,
/// (group_seq, frame_idx), and codec hints
pub struct AadBuilder {
    parts: Vec<Vec<u8>>,
}

impl AadBuilder {
    pub fn new() -> Self {
        Self { parts: Vec::new() }
    }

    pub fn version(mut self, version: u8) -> Self {
        self.parts.push(vec![version]);
        self
    }

    pub fn group_root(mut self, root: &str) -> Self {
        self.parts.push(root.as_bytes().to_vec());
        self
    }

    pub fn track_label(mut self, label: &str) -> Self {
        self.parts.push(label.as_bytes().to_vec());
        self
    }

    pub fn epoch(mut self, epoch: u64) -> Self {
        self.parts.push(epoch.to_be_bytes().to_vec());
        self
    }

    pub fn group_sequence(mut self, seq: u64) -> Self {
        self.parts.push(seq.to_be_bytes().to_vec());
        self
    }

    pub fn frame_index(mut self, idx: u64) -> Self {
        self.parts.push(idx.to_be_bytes().to_vec());
        self
    }

    pub fn keyframe(mut self, is_keyframe: bool) -> Self {
        self.parts.push(vec![if is_keyframe { 1 } else { 0 }]);
        self
    }

    pub fn build(self) -> Vec<u8> {
        self.parts.concat()
    }
}

impl Default for AadBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let base_key = [42u8; 32];
        let mut crypto = MediaCrypto::new(base_key);

        let plaintext = b"Hello, encrypted world!";
        let frame_counter = 12345;
        let aad = b"test-aad";

        let ciphertext = crypto
            .encrypt(plaintext, frame_counter, aad)
            .expect("encrypt");
        assert_ne!(ciphertext, plaintext);

        let decrypted = crypto
            .decrypt(&ciphertext, frame_counter, aad)
            .expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_wrong_aad_fails() {
        let base_key = [42u8; 32];
        let mut crypto = MediaCrypto::new(base_key);

        let plaintext = b"secret data";
        let frame_counter = 100;
        let aad = b"correct-aad";

        let ciphertext = crypto.encrypt(plaintext, frame_counter, aad).unwrap();

        // Decryption with wrong AAD should fail
        let wrong_aad = b"wrong-aad";
        let result = crypto.decrypt(&ciphertext, frame_counter, wrong_aad);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_wrong_counter_fails() {
        let base_key = [42u8; 32];
        let mut crypto = MediaCrypto::new(base_key);

        let plaintext = b"secret data";
        let frame_counter = 100;
        let aad = b"test-aad";

        let ciphertext = crypto.encrypt(plaintext, frame_counter, aad).unwrap();

        // Decryption with wrong counter should fail (different nonce)
        let result = crypto.decrypt(&ciphertext, frame_counter + 1, aad);
        assert!(result.is_err());
    }

    #[test]
    fn test_generation_rollover() {
        let base_key = [1u8; 32];
        let mut crypto = MediaCrypto::new(base_key);

        let plaintext = b"test";
        let aad = b"aad";

        // Generation 0 (counter MSB = 0)
        let counter_gen0 = 0x00_FF_FF_FF; // gen=0, counter=16777215
        let ct_gen0 = crypto.encrypt(plaintext, counter_gen0, aad).unwrap();
        let pt_gen0 = crypto.decrypt(&ct_gen0, counter_gen0, aad).unwrap();
        assert_eq!(pt_gen0, plaintext);

        // Generation 1 (counter MSB = 1)
        let counter_gen1 = 0x01_00_00_00; // gen=1, counter=16777216
        let ct_gen1 = crypto.encrypt(plaintext, counter_gen1, aad).unwrap();
        let pt_gen1 = crypto.decrypt(&ct_gen1, counter_gen1, aad).unwrap();
        assert_eq!(pt_gen1, plaintext);

        // Ciphertexts should differ (different generation keys)
        assert_ne!(ct_gen0, ct_gen1);
    }

    #[test]
    fn test_key_caching() {
        let base_key = [7u8; 32];
        let mut crypto = MediaCrypto::new(base_key);

        let plaintext = b"cached";
        let aad = b"test";

        // Encrypt twice with same generation - should use cached key
        let counter1 = 0x05_00_00_01; // gen=5
        let counter2 = 0x05_00_00_02; // gen=5

        crypto.encrypt(plaintext, counter1, aad).unwrap();
        assert_eq!(crypto.key_cache.len(), 1);

        crypto.encrypt(plaintext, counter2, aad).unwrap();
        assert_eq!(crypto.key_cache.len(), 1); // Still cached

        // Different generation should create new cache entry
        let counter3 = 0x06_00_00_01; // gen=6
        crypto.encrypt(plaintext, counter3, aad).unwrap();
        assert_eq!(crypto.key_cache.len(), 2);
    }

    #[test]
    fn test_aad_builder() {
        let aad = AadBuilder::new()
            .version(1)
            .group_root("marmot/abc123")
            .track_label("track001")
            .epoch(5)
            .group_sequence(100)
            .frame_index(42)
            .keyframe(true)
            .build();

        assert!(!aad.is_empty());
        assert!(aad.len() > 10); // Should contain all components

        // AAD should be deterministic
        let aad2 = AadBuilder::new()
            .version(1)
            .group_root("marmot/abc123")
            .track_label("track001")
            .epoch(5)
            .group_sequence(100)
            .frame_index(42)
            .keyframe(true)
            .build();
        assert_eq!(aad, aad2);
    }

    #[test]
    fn test_different_base_keys_produce_different_ciphertexts() {
        let base_key1 = [1u8; 32];
        let base_key2 = [2u8; 32];

        let mut crypto1 = MediaCrypto::new(base_key1);
        let mut crypto2 = MediaCrypto::new(base_key2);

        let plaintext = b"same plaintext";
        let counter = 100;
        let aad = b"aad";

        let ct1 = crypto1.encrypt(plaintext, counter, aad).unwrap();
        let ct2 = crypto2.encrypt(plaintext, counter, aad).unwrap();

        // Different base keys should produce different ciphertexts
        assert_ne!(ct1, ct2);

        // Each should decrypt with its own crypto
        assert_eq!(crypto1.decrypt(&ct1, counter, aad).unwrap(), plaintext);
        assert_eq!(crypto2.decrypt(&ct2, counter, aad).unwrap(), plaintext);

        // Cross-decryption should fail
        assert!(crypto1.decrypt(&ct2, counter, aad).is_err());
        assert!(crypto2.decrypt(&ct1, counter, aad).is_err());
    }
}
