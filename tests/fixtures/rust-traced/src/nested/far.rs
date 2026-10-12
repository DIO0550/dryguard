pub struct Remote;
impl super::Deep for Remote {
    type Out = i32;
    type Me = Self;
    type Wrap<U> = U;
}
