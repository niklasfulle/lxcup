use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

uuid_id!(NodeId);
uuid_id!(TargetId);
uuid_id!(EnvironmentId);
uuid_id!(EnrollmentId);
uuid_id!(ScanId);
uuid_id!(SecretId);
uuid_id!(AnsibleJobId);
uuid_id!(AgentRegistrationId);
uuid_id!(UpdatePlanId);
uuid_id!(ExecutionId);

/// Numerische Kennung eines historischen LXC-Datensatzes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ContainerId(u64);

impl ContainerId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}
