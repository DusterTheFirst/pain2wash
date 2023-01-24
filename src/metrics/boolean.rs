use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};

use prometheus_client::{
    encoding::{EncodeMetric, MetricEncoder},
    metrics::{MetricType, TypedMetric},
};

#[derive(Debug, Default, Clone)]
pub struct BooleanGauge(Arc<AtomicBool>);

impl BooleanGauge {
    pub fn set(&self, value: bool) {
        self.0.store(value, Ordering::SeqCst);
    }

    pub fn toggle(&self) {
        self.0.fetch_xor(true, Ordering::SeqCst);
    }
}

impl TypedMetric for BooleanGauge {
    const TYPE: MetricType = MetricType::Gauge;
}

impl EncodeMetric for BooleanGauge {
    fn encode(&self, mut encoder: MetricEncoder) -> Result<(), std::fmt::Error> {
        encoder.encode_gauge(&i64::from(u8::from(self.0.load(Ordering::SeqCst))))
    }

    fn metric_type(&self) -> MetricType {
        Self::TYPE
    }
}

use crate::pay2wash::model::NumberBool;

#[derive(Debug, Default, Clone)]
pub struct NumberBooleanGauge(Arc<AtomicU8>);

impl NumberBooleanGauge {
    pub fn set(&self, value: NumberBool) {
        self.0.store(value.into(), Ordering::SeqCst);
    }
}

impl TypedMetric for NumberBooleanGauge {
    const TYPE: MetricType = MetricType::Gauge;
}

impl EncodeMetric for NumberBooleanGauge {
    fn encode(&self, mut encoder: MetricEncoder) -> Result<(), std::fmt::Error> {
        encoder.encode_gauge(&i64::from(self.0.load(Ordering::SeqCst)))
    }

    fn metric_type(&self) -> MetricType {
        Self::TYPE
    }
}
