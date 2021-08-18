/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0.
 */

use std::sync::Arc;

use aws_auth::provider::{AsyncProvideCredentials, CredentialsError, CredentialsResult};
use aws_auth::Credentials;
use aws_hyper::StandardClient;
use aws_sdk_sts::operation::AssumeRole;
use aws_sdk_sts::Config;
use aws_types::region::Region;

use crate::profile::repr::BaseProvider;
use crate::profile::ProfileFileError;

use super::repr;
use std::fmt::{Debug, Formatter};

#[derive(Debug)]
pub struct AssumeRoleProvider {
    role_arn: String,
    external_id: Option<String>,
    session_name: Option<String>,
}

pub struct ClientConfiguration {
    pub core_client: StandardClient,
    pub region: Option<Region>,
}

impl AssumeRoleProvider {
    pub async fn credentials(
        &self,
        input_credentials: Credentials,
        client_config: &ClientConfiguration,
    ) -> CredentialsResult {
        let config = Config::builder()
            .credentials_provider(input_credentials)
            .region(client_config.region.clone())
            .build();
        let operation = AssumeRole::builder()
            .role_arn(&self.role_arn)
            .set_external_id(self.external_id.clone())
            .role_session_name(
                self.session_name
                    .as_deref()
                    .unwrap_or("assume-role-provider-session"),
            )
            .build()
            .expect("operation is valid")
            .make_operation(&config)
            .expect("valid operation");
        let assume_role_creds = client_config
            .core_client
            .call(operation)
            .await
            .map_err(|err| CredentialsError::Unhandled(err.into()))?
            .credentials
            .ok_or_else(|| {
                CredentialsError::Unhandled(
                    "assume role provider did not return credentials".into(),
                )
            })?;
        let expiration = assume_role_creds
            .expiration
            .ok_or_else(|| CredentialsError::Unhandled("missing expiration".into()))?;
        let expiration = expiration.to_system_time().ok_or_else(|| {
            CredentialsError::Unhandled(
                format!("expiration is before unix epoch: {:?}", &expiration).into(),
            )
        })?;
        Ok(Credentials::new(
            assume_role_creds.access_key_id.ok_or_else(|| {
                CredentialsError::Unhandled("access key id missing from result".into())
            })?,
            assume_role_creds
                .secret_access_key
                .ok_or_else(|| CredentialsError::Unhandled("secret access token missing".into()))?,
            assume_role_creds.session_token,
            Some(expiration),
            "AssumeRoleProvider",
        ))
    }
}

pub(crate) struct ProviderChain {
    base: Arc<dyn AsyncProvideCredentials>,
    chain: Vec<AssumeRoleProvider>,
}

impl Debug for ProviderChain {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // TODO: AsyncProvideCredentials should probably mandate debug
        f.debug_struct("ProviderChain").finish()
    }
}

impl ProviderChain {
    pub fn base(&self) -> &dyn AsyncProvideCredentials {
        self.base.as_ref()
    }

    pub fn chain(&self) -> &[AssumeRoleProvider] {
        &self.chain.as_slice()
    }
}

impl ProviderChain {
    pub fn from_repr(
        repr: repr::ProfileChain,
        factory: &named::NamedProviderFactory,
    ) -> Result<Self, ProfileFileError> {
        let base = match repr.base() {
            BaseProvider::NamedSource(name) => {
                factory
                    .provider(name)
                    .ok_or(ProfileFileError::UnknownProvider {
                        name: name.to_string(),
                    })?
            }
            BaseProvider::AccessKey(key) => Arc::new(key.clone()),
        };
        tracing::info!(base = ?repr.base(), "first credentials will be loaded from {:?}", repr.base());
        let chain = repr
            .chain()
            .iter()
            .map(|role_arn| {
                tracing::info!(role_arn = ?role_arn, "which will be used to assume a role");
                AssumeRoleProvider {
                    role_arn: role_arn.role_arn.into(),
                    external_id: role_arn.external_id.map(|id| id.into()),
                    session_name: role_arn.session_name.map(|id| id.into()),
                }
            })
            .collect();
        Ok(ProviderChain { base, chain })
    }
}

pub mod named {
    use std::collections::HashMap;
    use std::sync::Arc;

    use aws_auth::provider::AsyncProvideCredentials;
    use std::borrow::Cow;

    pub struct NamedProviderFactory {
        providers: HashMap<Cow<'static, str>, Arc<dyn AsyncProvideCredentials>>,
    }

    impl NamedProviderFactory {
        pub fn new(
            providers: HashMap<Cow<'static, str>, Arc<dyn AsyncProvideCredentials>>,
        ) -> Self {
            Self { providers }
        }

        pub fn provider(&self, name: &str) -> Option<Arc<dyn AsyncProvideCredentials>> {
            self.providers.get(name).cloned()
        }
    }
}

#[cfg(test)]
mod test {
    use crate::profile::exec::named::NamedProviderFactory;
    use crate::profile::exec::ProviderChain;
    use crate::profile::repr::{BaseProvider, ProfileChain};
    use std::collections::HashMap;

    #[test]
    fn error_on_unknown_provider() {
        let factory = NamedProviderFactory::new(HashMap::new());
        let chain = ProviderChain::from_repr(
            ProfileChain {
                base: BaseProvider::NamedSource("floozle"),
                chain: vec![],
            },
            &factory,
        );
        let err = chain.expect_err("no source by that name");
        assert!(
            format!("{}", err).contains(
                "profile referenced `floozle` provider but that provider is not supported"
            ),
            "`{}` did not match expected error",
            err
        );
    }
}