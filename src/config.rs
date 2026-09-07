//! Source types accepted by the portable mdoc import/export codec.
pub fn builtin_srctypes() -> impl ExactSizeIterator<Item = &'static str> {
    crate::store::BLOCK_TYPES.into_iter()
}
pub fn builtin_srctype(srctype: &str) -> anyhow::Result<&'static str> {
    builtin_srctypes()
        .find(|known| known.eq_ignore_ascii_case(srctype))
        .ok_or_else(|| {
            anyhow::anyhow!("unsupported srctype '{srctype}'; expected text, lean, rocq or latex")
        })
}
pub fn canonical_srctype(srctype: &str) -> &str {
    builtin_srctype(srctype).unwrap_or(srctype)
}
