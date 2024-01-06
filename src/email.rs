use crate::config;
use crate::Env;
use lettre::address::Envelope;
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::file::FileTransport;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::SmtpTransport;
use lettre::transport::stub::StubTransport;
use lettre::{Message, Transport};
use rand::distributions::{Alphanumeric, DistString};

pub trait Email {
    const SUBJECT: &'static str;
    fn body(&self) -> String;
}

#[derive(Debug, Clone)]
pub struct Emails {
    backend: EmailBackend,
    pub domain: String,
    from: Mailbox,
}

const DEFAULT_FROM: &str = "noreply@crates.io";

impl Emails {
    /// Create a new instance detecting the backend from the environment. This will either connect
    /// to a SMTP server or store the emails on the local filesystem.
    pub fn from_environment(config: &config::Server) -> Self {
        let login = dotenvy::var("MAILGUN_SMTP_LOGIN");
        let password = dotenvy::var("MAILGUN_SMTP_PASSWORD");
        let server = dotenvy::var("MAILGUN_SMTP_SERVER");

        let from = login.as_deref().unwrap_or(DEFAULT_FROM).parse().unwrap();

        let backend = match (login, password, server) {
            (Ok(login), Ok(password), Ok(server)) => {
                let transport = SmtpTransport::relay(&server)
                    .unwrap()
                    .credentials(Credentials::new(login, password))
                    .authentication(vec![Mechanism::Plain])
                    .build();

                EmailBackend::Smtp(Box::new(transport))
            }
            _ => {
                let transport = FileTransport::new("/tmp");
                EmailBackend::FileSystem(transport)
            }
        };

        if config.base.env == Env::Production && !matches!(backend, EmailBackend::Smtp { .. }) {
            panic!("only the smtp backend is allowed in production");
        }

        let domain = config.domain_name.clone();

        Self {
            backend,
            domain,
            from,
        }
    }

    /// Create a new test backend that stores all the outgoing emails in memory, allowing for tests
    /// to later assert the mails were sent.
    pub fn new_in_memory() -> Self {
        Self {
            backend: EmailBackend::Memory(StubTransport::new_ok()),
            domain: "crates.io".into(),
            from: DEFAULT_FROM.parse().unwrap(),
        }
    }

    /// This is supposed to be used only during tests, to retrieve the messages stored in the
    /// "memory" backend. It's not cfg'd away because our integration tests need to access this.
    pub fn mails_in_memory(&self) -> Option<Vec<(Envelope, String)>> {
        if let EmailBackend::Memory(transport) = &self.backend {
            Some(transport.messages())
        } else {
            None
        }
    }

    pub fn send<E: Email>(&self, recipient: &str, email: E) -> Result<(), EmailError> {
        // The message ID is normally generated by the SMTP server, but if we let it generate the
        // ID there will be no way for the crates.io application to know the ID of the message it
        // just sent, as it's not included in the SMTP response.
        //
        // Our support staff needs to know the message ID to be able to find misdelivered emails.
        // Because of that we're generating a random message ID, hoping the SMTP server doesn't
        // replace it when it relays the message.
        let message_id = format!(
            "<{}@{}>",
            Alphanumeric.sample_string(&mut rand::thread_rng(), 32),
            self.domain,
        );

        let subject = E::SUBJECT;
        let body = email.body();

        let email = Message::builder()
            .message_id(Some(message_id.clone()))
            .to(recipient.parse()?)
            .from(self.from.clone())
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body)?;

        self.backend.send(email).map_err(EmailError::TransportError)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error(transparent)]
    AddressError(#[from] lettre::address::AddressError),
    #[error(transparent)]
    MessageBuilderError(#[from] lettre::error::Error),
    #[error(transparent)]
    TransportError(anyhow::Error),
}

#[derive(Debug, Clone)]
enum EmailBackend {
    /// Backend used in production to send mails using SMTP.
    ///
    /// This is using `Box` to avoid a large size difference between variants.
    Smtp(Box<SmtpTransport>),
    /// Backend used locally during development, will store the emails in the provided directory.
    FileSystem(FileTransport),
    /// Backend used during tests, will keep messages in memory to allow tests to retrieve them.
    Memory(StubTransport),
}

impl EmailBackend {
    fn send(&self, message: Message) -> anyhow::Result<()> {
        match self {
            EmailBackend::Smtp(transport) => transport.send(&message).map(|_| ())?,
            EmailBackend::FileSystem(transport) => transport.send(&message).map(|_| ())?,
            EmailBackend::Memory(transport) => transport.send(&message).map(|_| ())?,
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct StoredEmail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestEmail;

    impl Email for TestEmail {
        const SUBJECT: &'static str = "test";

        fn body(&self) -> String {
            "test".into()
        }
    }

    #[test]
    fn sending_to_invalid_email_fails() {
        let emails = Emails::new_in_memory();

        assert_err!(emails.send(
            "String.Format(\"{0}.{1}@live.com\", FirstName, LastName)",
            TestEmail
        ));
    }

    #[test]
    fn sending_to_valid_email_succeeds() {
        let emails = Emails::new_in_memory();

        assert_ok!(emails.send("someone@example.com", TestEmail));
    }
}
