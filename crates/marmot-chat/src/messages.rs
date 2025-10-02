#[derive(Clone, Debug)]
pub struct WrapperFrame {
    pub bytes: Vec<u8>,
    pub kind: WrapperKind,
}

#[derive(Clone, Debug)]
pub enum WrapperKind {
    Application { author: String, content: String },
    Commit,
}

impl WrapperKind {
    pub fn label(&self) -> &'static str {
        match self {
            WrapperKind::Application { .. } => "application",
            WrapperKind::Commit => "commit",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            WrapperKind::Application { author, content } => {
                format!("{author}: {content}")
            }
            WrapperKind::Commit => "commit".to_string(),
        }
    }
}
