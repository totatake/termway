//! SIMD-accelerated JPEG primitives (AVX2 for x86_64, NEON for aarch64).
//!
//! Maintains strict bit-exact equivalence (INV-4) with scalar JPEG output.

#[cfg(target_arch = "x86_64")]
pub mod avx2;

#[cfg(target_arch = "aarch64")]
pub mod neon;

/// Hardware acceleration backend for JPEG encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegBackend {
    Scalar,
    Avx2,
    Neon,
}

impl JpegBackend {
    /// Detects available SIMD features on the current host.
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("avx2") {
                return Self::Avx2;
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            return Self::Neon;
        }
        #[allow(unreachable_code)]
        Self::Scalar
    }

    /// Checks the `TERMWAY_JPEG_SIMD` environment variable or detects automatically.
    ///
    /// If `TERMWAY_JPEG_SIMD=0`, forces `Scalar`.
    pub fn from_env_or_detect() -> Self {
        if let Ok(val) = std::env::var("TERMWAY_JPEG_SIMD") {
            let v = val.trim();
            if v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("scalar") {
                return Self::Scalar;
            }
            #[cfg(target_arch = "x86_64")]
            if v.eq_ignore_ascii_case("avx2") && std::is_x86_feature_detected!("avx2") {
                return Self::Avx2;
            }
            #[cfg(target_arch = "aarch64")]
            if v.eq_ignore_ascii_case("neon") {
                return Self::Neon;
            }
        }
        Self::detect()
    }
}
