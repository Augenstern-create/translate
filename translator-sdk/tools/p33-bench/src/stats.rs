//! 分位数：用最近秩法（nearest-rank），不做插值。
//!
//! 理由：样本量固定（n=100/500）、要能被人手工复核。"p95 = 第 95 个排序样本"比
//! "线性插值得到的 13.237 ms"更容易验证，也更难被无意中"优化"。

#[derive(Clone, Debug, Default)]
pub struct Sample {
    values: Vec<f64>,
}

impl Sample {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, v: f64) {
        self.values.push(v);
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn percentile(&self, p: f64) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        let mut sorted = self.values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
        let idx = rank.saturating_sub(1).min(sorted.len() - 1);
        sorted[idx]
    }
    pub fn min(&self) -> f64 {
        self.percentile(0.0)
    }
    pub fn max(&self) -> f64 {
        self.values.iter().cloned().fold(0.0, f64::max)
    }
    pub fn mean(&self) -> f64 {
        if self.values.is_empty() {
            0.0
        } else {
            self.values.iter().sum::<f64>() / self.values.len() as f64
        }
    }
    pub fn to_json(&self, unit: &str) -> serde_json::Value {
        serde_json::json!({
            "n": self.len(),
            "p50": round2(self.percentile(50.0)),
            "p95": round2(self.percentile(95.0)),
            "p99": round2(self.percentile(99.0)),
            "min": round2(self.min()),
            "max": round2(self.max()),
            "mean": round2(self.mean()),
            "unit": unit,
        })
    }
}

pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
