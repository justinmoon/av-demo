use marmot_chat::messages::{WrapperFrame, WrapperKind};

pub mod scenario {
    use super::WrapperFrame;
    use super::WrapperKind;

    include!("scenario.rs");
}
