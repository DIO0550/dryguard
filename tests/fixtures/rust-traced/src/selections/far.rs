pub trait Far {
    type Out;
}
impl<Y> Far for super::Holder<Y> {
    type Out = Vec<Y>;
}
