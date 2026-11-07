//! `pgrustadmin-server user create`: provisions login users from the CLI.

use std::{
    ffi::OsString,
    io::{self, BufRead},
};

use pgrustadmin_app_core::EmailAddress;
use pgrustadmin_postgres_driver::{PgPool, connect_metadata_pool, create_user as insert_user};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{auth::password::hash_password, connections::is_unique_violation};

pub const USAGE: &str =
    "usage: pgrustadmin-server [migrate | user create --email <email> [--password-stdin]]";
const MIN_PASSWORD_CHARS: usize = 12;
const MAX_PASSWORD_CHARS: usize = 1024;
const MAX_EMAIL_BYTES: usize = 254;

#[derive(Debug, PartialEq, Eq)]
pub struct CreateUser {
    email: String,
    password_from_stdin: bool,
}

/// Parses the arguments after `user`.
pub fn parse_user_command(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CreateUser, &'static str> {
    if arguments.next().as_deref() != Some("create".as_ref()) {
        return Err(USAGE);
    }
    let mut email = None;
    let mut password_from_stdin = false;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--email") if email.is_none() => {
                let value = arguments.next().ok_or(USAGE)?;
                email = Some(
                    value
                        .into_string()
                        .map_err(|_| "email must be valid UTF-8")?,
                );
            }
            Some("--password-stdin") if !password_from_stdin => password_from_stdin = true,
            _ => return Err(USAGE),
        }
    }
    Ok(CreateUser {
        email: email.ok_or(USAGE)?,
        password_from_stdin,
    })
}

/// Normalizes the email and checks a minimal address shape.
pub fn validate_email(value: &str) -> Result<EmailAddress, &'static str> {
    const INVALID: &str = "email must look like name@example.com";
    let email = EmailAddress::new(value).map_err(|_| INVALID)?;
    let text = email.as_str();
    let Some((local, domain)) = text.split_once('@') else {
        return Err(INVALID);
    };
    let valid = text.len() <= MAX_EMAIL_BYTES
        && !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !text.chars().any(char::is_whitespace);
    if valid { Ok(email) } else { Err(INVALID) }
}

pub fn validate_password(password: &str) -> Result<(), &'static str> {
    let length = password.chars().count();
    if (MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&length) {
        Ok(())
    } else {
        Err("password must have from 12 to 1024 characters")
    }
}

/// Reads the first line of `reader` as the password, without its line ending.
pub fn read_password_line(mut reader: impl BufRead) -> io::Result<Zeroizing<String>> {
    let mut line = Zeroizing::new(String::new());
    reader.read_line(&mut line)?;
    let trimmed = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(trimmed);
    Ok(line)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CreateUserError {
    Duplicate,
    Hashing,
    Database,
}

impl CreateUserError {
    fn message(&self) -> &'static str {
        match self {
            Self::Duplicate => "a user with this email already exists",
            Self::Hashing => "could not hash the password",
            Self::Database => "could not create the user in the metadata database",
        }
    }
}

/// Hashes the password off the async workers and inserts an active user.
pub async fn create_user(
    pool: &PgPool,
    email: &EmailAddress,
    password: Zeroizing<String>,
) -> Result<Uuid, CreateUserError> {
    let hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| CreateUserError::Hashing)?
        .map_err(|_| CreateUserError::Hashing)?;
    let user_id = Uuid::new_v4();
    match insert_user(pool, user_id, email.as_str(), hash.as_encoded()).await {
        Ok(()) => Ok(user_id),
        Err(error) if is_unique_violation(&error) => Err(CreateUserError::Duplicate),
        Err(_) => Err(CreateUserError::Database),
    }
}

/// Runs `user ...` with the remaining command-line arguments.
pub async fn run(arguments: impl Iterator<Item = OsString>) -> io::Result<()> {
    let command = parse_user_command(arguments).map_err(invalid_input)?;
    let email = validate_email(&command.email).map_err(invalid_input)?;
    let password = if command.password_from_stdin {
        read_password_line(io::stdin().lock())?
    } else {
        prompt_password()?
    };
    validate_password(&password).map_err(invalid_input)?;

    let database_url = std::env::var("PGRUSTADMIN_METADATA_DATABASE_URL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "PGRUSTADMIN_METADATA_DATABASE_URL is required to create users",
        )
    })?;
    let pool = connect_metadata_pool(&database_url)
        .await
        .map_err(|_| io::Error::other("could not connect to metadata database"))?;
    let result = create_user(&pool, &email, password).await;
    pool.close().await;
    let user_id = result.map_err(|error| io::Error::other(error.message()))?;
    println!("created user {user_id}");
    Ok(())
}

fn prompt_password() -> io::Result<Zeroizing<String>> {
    let password = Zeroizing::new(rpassword::prompt_password("Password: ")?);
    let confirmation = Zeroizing::new(rpassword::prompt_password("Confirm password: ")?);
    if *password == *confirmation {
        Ok(password)
    } else {
        Err(invalid_input("passwords do not match"))
    }
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsString};

    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
    };
    use pgrustadmin_postgres_driver::{connect_metadata_pool, run_metadata_migrations};
    use tower::ServiceExt;
    use uuid::Uuid;
    use zeroize::Zeroizing;

    use super::{
        CreateUser, CreateUserError, USAGE, create_user, parse_user_command, read_password_line,
        validate_email, validate_password,
    };

    fn args(values: &[&str]) -> impl Iterator<Item = OsString> {
        values
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_create_with_email_and_optional_stdin_flag() {
        assert_eq!(
            parse_user_command(args(&["create", "--email", "a@b.io"])),
            Ok(CreateUser {
                email: "a@b.io".to_owned(),
                password_from_stdin: false,
            })
        );
        assert_eq!(
            parse_user_command(args(&["create", "--password-stdin", "--email", "a@b.io"])),
            Ok(CreateUser {
                email: "a@b.io".to_owned(),
                password_from_stdin: true,
            })
        );
        for invalid in [
            &[][..],
            &["delete", "--email", "a@b.io"][..],
            &["create"][..],
            &["create", "--email"][..],
            &["create", "--email", "a@b.io", "--email", "c@d.io"][..],
            &["create", "--email", "a@b.io", "--password", "secret"][..],
        ] {
            assert_eq!(parse_user_command(args(invalid)), Err(USAGE), "{invalid:?}");
        }
    }

    #[test]
    fn emails_are_normalized_and_need_a_minimal_shape() {
        assert_eq!(
            validate_email("  Ana@Example.COM ").map(|email| email.as_str().to_owned()),
            Ok("ana@example.com".to_owned())
        );
        let long = format!("{}@example.com", "a".repeat(243));
        for invalid in [
            "",
            "ana",
            "@example.com",
            "ana@",
            "ana@example",
            "ana@@example.com",
            "a@b@example.com",
            "ana maria@example.com",
            "ana@.com",
            "ana@example.",
            long.as_str(),
        ] {
            assert!(validate_email(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn passwords_need_twelve_to_1024_characters() {
        assert!(validate_password("short-pass1").is_err());
        assert!(validate_password("twelve-chars").is_ok());
        assert!(
            validate_password("çççççççççççç").is_ok(),
            "counts characters"
        );
        assert!(validate_password(&"x".repeat(1024)).is_ok());
        assert!(validate_password(&"x".repeat(1025)).is_err());
    }

    #[test]
    fn stdin_password_is_the_first_line_without_its_ending() {
        let read = |input: &str| read_password_line(input.as_bytes()).expect("read line");
        assert_eq!(
            read("secret value 1\r\nignored\n").as_str(),
            "secret value 1"
        );
        assert_eq!(read("secret value 2\n").as_str(), "secret value 2");
        assert_eq!(read("  spaced  ").as_str(), "  spaced  ");
        assert_eq!(read("").as_str(), "");
    }

    #[tokio::test]
    async fn created_users_can_log_in_and_emails_stay_unique() {
        let database_url = env::var("PGRUSTADMIN_TEST_DATABASE_URL")
            .expect("PGRUSTADMIN_TEST_DATABASE_URL is required for database tests");
        let pool = connect_metadata_pool(&database_url)
            .await
            .expect("connect to the isolated test metadata database");
        run_metadata_migrations(&pool)
            .await
            .expect("apply metadata migrations to the test database");

        let email = validate_email(&format!("Created-{}@Example.test", Uuid::new_v4()))
            .expect("valid email");
        let password = "correct horse battery";
        let user_id = create_user(&pool, &email, Zeroizing::new(password.to_owned()))
            .await
            .expect("create user");
        assert_eq!(
            create_user(&pool, &email, Zeroizing::new(password.to_owned())).await,
            Err(CreateUserError::Duplicate)
        );

        let stored: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("stored hash");
        assert!(stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(!stored.contains(password));

        let login = crate::auth::handlers::router(pool.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "email": email.as_str(), "password": password })
                            .to_string(),
                    ))
                    .expect("build login request"),
            )
            .await
            .expect("login responds");
        assert_eq!(login.status(), StatusCode::OK);
        let body = to_bytes(login.into_body(), 4096)
            .await
            .expect("read login body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("decode login body");
        assert_eq!(body["user_id"], user_id.to_string());
    }
}
