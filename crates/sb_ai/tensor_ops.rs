use ndarray::{Array2, ArrayView3, Axis};

pub(crate) fn mean_pool(
    last_hidden_states: ArrayView3<f32>,
    attention_mask: ArrayView3<i64>,
) -> Array2<f32> {
    let masked_hidden_states = last_hidden_states.into_owned() * &attention_mask.mapv(|x| x as f32);
    let sum_hidden_states = masked_hidden_states.sum_axis(Axis(1));
    let sum_attention_mask = attention_mask.mapv(|x| x as f32).sum_axis(Axis(1));

    sum_hidden_states / sum_attention_mask
}
