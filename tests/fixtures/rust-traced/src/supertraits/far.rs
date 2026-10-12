pub trait FarSub: super::Show {
    fn distant(value: Self::Item) -> u8;
    fn distant_plain(value: u16) -> u8;
}
