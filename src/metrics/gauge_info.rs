use std::hash::Hash;

use prometheus_client::{
    encoding::{EncodeLabelSet, EncodeMetric, MetricEncoder},
    metrics::{MetricType, TypedMetric},
};

#[derive(Debug)]
pub struct GaugeInfo<S>(S)
where
    S: Clone + Hash + Eq + EncodeLabelSet;

impl<S> GaugeInfo<S>
where
    S: Clone + Hash + Eq + EncodeLabelSet,
{
    pub fn new(label_set: S) -> Self {
        Self(label_set)
    }
}

impl<S> TypedMetric for GaugeInfo<S>
where
    S: Clone + Hash + Eq + EncodeLabelSet,
{
    const TYPE: MetricType = MetricType::Gauge;
}

impl<S> EncodeMetric for GaugeInfo<S>
where
    S: Clone + Hash + Eq + EncodeLabelSet,
{
    fn encode(&self, mut encoder: MetricEncoder) -> Result<(), std::fmt::Error> {
        encoder.encode_info(&self.0)
    }

    fn metric_type(&self) -> MetricType {
        Self::TYPE
    }
}
