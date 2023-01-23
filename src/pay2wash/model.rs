use serde::{
    de::{self, Visitor},
    Deserialize,
};
use thiserror::Error;

use std::{
    fmt::{self, Debug},
    time::Duration,
};

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct JsonMachineStatus {
    pub running: bool,
    pub starter: UserId,
    pub reserved: bool,
    pub reserver: UserId,
    pub in_maintenance: NumberBool,
    pub remaining_time: RemainingTime,
    pub gateway_offline: NumberBool,
    pub remaining_time_is_from_machine: NumberBool,
    // Unknown
    pub controller_logic: u32,
}

pub mod influx {
    use std::time::SystemTime;

    use influxdb::{InfluxDbWriteable, Timestamp};

    use super::{JsonMachineStatus, NumberBool, RemainingTime, UserId};

    #[derive(Debug, InfluxDbWriteable)]
    pub struct InfluxMachineStatus<'str> {
        pub time: Timestamp,
        #[influxdb(tag)]
        pub machine_name: &'str str,
        #[influxdb(tag)]
        pub location: &'str str,
        pub running: bool,
        pub starter: UserId,
        pub reserved: bool,
        pub reserver: UserId,
        pub in_maintenance: NumberBool,
        pub remaining_time: RemainingTime,
        pub gateway_offline: NumberBool,
        pub remaining_time_is_from_machine: NumberBool,
        // Unknown
        pub controller_logic: u32,
    }

    impl<'s> InfluxMachineStatus<'s> {
        pub fn new(
            JsonMachineStatus {
                running,
                starter,
                reserved,
                reserver,
                in_maintenance,
                remaining_time,
                gateway_offline,
                remaining_time_is_from_machine,
                controller_logic,
            }: JsonMachineStatus,
            machine_name: &'s str,
            location: &'s str,
        ) -> Self {
            Self {
                time: Timestamp::Milliseconds(
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .expect("Time has moved backwards")
                        .as_millis(),
                ),
                machine_name,
                location,
                running,
                starter,
                reserved,
                reserver,
                in_maintenance,
                remaining_time,
                gateway_offline,
                remaining_time_is_from_machine,
                controller_logic,
            }
        }
    }
}

#[derive(Debug)]
pub struct MachineStatus {
    pub state: MachineState,
    pub raw: JsonMachineStatus,
}

#[derive(Debug, Clone, Copy)]
pub enum MachineState {
    Running {
        starter: UserId,
        remaining_time: RemainingTime,
        remaining_time_is_from_machine: NumberBool,
    },
    Reserved {
        reserver: UserId,
    },
    Maintenance,
    Idle,
}

#[derive(Debug, Error)]
pub enum FromMachineStatusError {
    #[error("attempted to interpret in_maintenance and received an unknown value: {0}")]
    UnknownNumberBool(u8),
    #[error("invariant does not hold: running ({running}) reserved ({running}), in_maintenance ({in_maintenance:?})")]
    BadInvariant {
        running: bool,
        reserved: bool,
        in_maintenance: NumberBool,
    },
}

impl TryFrom<&JsonMachineStatus> for MachineState {
    type Error = FromMachineStatusError;

    fn try_from(status: &JsonMachineStatus) -> Result<Self, Self::Error> {
        match (status.running, status.reserved, status.in_maintenance) {
            (true, _, NumberBool::False) => Ok(Self::Running {
                starter: status.starter,
                remaining_time: status.remaining_time,
                remaining_time_is_from_machine: status.remaining_time_is_from_machine,
            }),
            (false, true, NumberBool::False) => Ok(Self::Reserved {
                reserver: status.reserver,
            }),
            (false, false, NumberBool::True) => Ok(Self::Maintenance),
            (false, false, NumberBool::False) => Ok(Self::Idle),
            (_, _, NumberBool::Unknown(bool)) => {
                Err(FromMachineStatusError::UnknownNumberBool(bool))
            }
            (running, reserved, in_maintenance) => Err(FromMachineStatusError::BadInvariant {
                running,
                reserved,
                in_maintenance,
            }),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(transparent)]
pub struct UserId(u32);

impl From<UserId> for influxdb::Type {
    fn from(value: UserId) -> Self {
        influxdb::Type::UnsignedInteger(value.0.into())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NumberBool {
    False,
    True,
    Unknown(u8),
}

impl From<NumberBool> for influxdb::Type {
    fn from(value: NumberBool) -> Self {
        influxdb::Type::UnsignedInteger(u8::from(value).into())
    }
}

impl From<u8> for NumberBool {
    fn from(value: u8) -> Self {
        match value {
            0 => NumberBool::False,
            1 => NumberBool::True,
            _ => NumberBool::Unknown(value),
        }
    }
}

impl From<NumberBool> for u8 {
    fn from(value: NumberBool) -> Self {
        match value {
            NumberBool::False => 0,
            NumberBool::True => 1,
            NumberBool::Unknown(value) => value,
        }
    }
}

impl TryFrom<NumberBool> for bool {
    type Error = u8;

    fn try_from(value: NumberBool) -> Result<Self, Self::Error> {
        match value {
            NumberBool::False => Ok(false),
            NumberBool::True => Ok(true),
            NumberBool::Unknown(unknown) => Err(unknown),
        }
    }
}

impl<'de> Deserialize<'de> for NumberBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct NumberBoolVisitor;

        impl<'v> Visitor<'v> for NumberBoolVisitor {
            type Value = u8;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("boolean represented as an integer")
            }

            fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(v)
            }

            fn visit_u16<E>(self, v: u16) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }

            fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }

            fn visit_u128<E>(self, v: u128) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                v.try_into().map_err(E::custom)
            }
        }

        deserializer
            .deserialize_u8(NumberBoolVisitor)
            .map(NumberBool::from)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RemainingTime(Duration);

impl From<RemainingTime> for influxdb::Type {
    fn from(value: RemainingTime) -> Self {
        influxdb::Type::UnsignedInteger(value.0.as_secs())
    }
}

impl RemainingTime {
    pub fn into_inner(self) -> Duration {
        self.0
    }

    pub fn inner(&self) -> &Duration {
        &self.0
    }
}

impl From<RemainingTime> for Duration {
    fn from(value: RemainingTime) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for RemainingTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RemainingTimeVisitor;

        impl<'v> Visitor<'v> for RemainingTimeVisitor {
            type Value = Duration;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a duration formatted as HH:MM")
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&v)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let (hours, minutes) = v
                    .split_once(':')
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(v), &self))?;

                let hours = (hours.len() <= 2).then_some(hours).ok_or_else(|| {
                    E::invalid_value(de::Unexpected::Str(v), &"too many digits in hours place")
                })?;
                let minutes = (minutes.len() <= 2).then_some(minutes).ok_or_else(|| {
                    E::invalid_value(de::Unexpected::Str(v), &"too many digits in minutes place")
                })?;

                let hours: u8 = hours.parse().map_err(E::custom)?;
                let minutes: u8 = minutes.parse().map_err(E::custom)?;

                Ok(Duration::from_secs(
                    60 * 60 * u64::from(hours) + 60 * u64::from(minutes),
                ))
            }
        }

        deserializer.deserialize_str(RemainingTimeVisitor).map(Self)
    }
}
