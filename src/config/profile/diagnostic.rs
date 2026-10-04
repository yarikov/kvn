#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    pub pointer: String,
    pub message: String,
}

impl ConfigDiagnostic {
    pub(crate) fn new(pointer: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
            message: message.into(),
        }
    }

    pub(crate) fn within(mut self, parent_pointer: &str) -> Self {
        self.pointer.insert_str(0, parent_pointer);
        self
    }

    pub(crate) fn labelled(mut self, label: &str) -> Self {
        self.message = format!("{label}: {}", self.message);
        self
    }
}

pub(crate) fn into_result(diagnostics: Vec<ConfigDiagnostic>) -> anyhow::Result<()> {
    if diagnostics.is_empty() {
        return Ok(());
    }
    let messages: Vec<_> = diagnostics.iter().map(|d| d.message.as_str()).collect();
    anyhow::bail!("{}", messages.join("; "))
}
