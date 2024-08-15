pub(super) fn ensure_vec_len<T: Default>(v: &mut Vec<T>, len: usize) {
    if v.len() < len {
        v.resize_with(len, T::default);
    }
}
