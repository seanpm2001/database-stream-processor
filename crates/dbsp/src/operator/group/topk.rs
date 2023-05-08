use super::{DiffGroupTransformer, Monotonicity, NonIncrementalGroupTransformer};
use crate::{
    algebra::ZRingValue, trace::Cursor, DBData, DBWeight, IndexedZSet, OrdIndexedZSet, RootCircuit,
    Stream,
};
use std::marker::PhantomData;

impl<B> Stream<RootCircuit, B>
where
    B: IndexedZSet + Send,
{
    pub fn topk_asc(&self, k: usize) -> Stream<RootCircuit, OrdIndexedZSet<B::Key, B::Val, B::R>>
    where
        B::R: ZRingValue,
    {
        self.group_transform(DiffGroupTransformer::new(TopK::asc(k)))
    }

    pub fn topk_desc(&self, k: usize) -> Stream<RootCircuit, OrdIndexedZSet<B::Key, B::Val, B::R>>
    where
        B::R: ZRingValue,
    {
        self.group_transform(DiffGroupTransformer::new(TopK::desc(k)))
    }
}

pub struct TopK<I, R> {
    k: usize,
    name: String,
    asc: bool,
    _phantom: PhantomData<(I, R)>,
}

impl<I, R> TopK<I, R> {
    fn asc(k: usize) -> Self {
        Self {
            k,
            name: format!("top-{k}-asc"),
            asc: true,
            _phantom: PhantomData,
        }
    }

    fn desc(k: usize) -> Self {
        Self {
            k,
            name: format!("top-{k}-desc"),
            asc: false,
            _phantom: PhantomData,
        }
    }
}

impl<I, R> NonIncrementalGroupTransformer<I, I, R> for TopK<I, R>
where
    I: DBData,
    R: DBWeight,
{
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn monotonicity(&self) -> Monotonicity {
        if self.asc {
            Monotonicity::Ascending
        } else {
            Monotonicity::Descending
        }
    }

    fn transform<C, CB>(&self, cursor: &mut C, mut output_cb: CB)
    where
        C: Cursor<I, (), (), R>,
        CB: FnMut(I, R),
    {
        let mut count = 0usize;

        if self.asc {
            while cursor.key_valid() && count < self.k {
                let w = cursor.weight();
                if !w.is_zero() {
                    output_cb(cursor.key().clone(), w);
                    count += 1;
                }
                cursor.step_key();
            }
        } else {
            cursor.fast_forward_keys();

            while cursor.key_valid() && count < self.k {
                let w = cursor.weight();
                if !w.is_zero() {
                    output_cb(cursor.key().clone(), w);
                    count += 1;
                }
                cursor.step_key_reverse();
            }
        }
    }
}
