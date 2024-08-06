use crate::config::PipelineConfig;
use crate::constants::BASE_IDS;
use crate::features::{Feature, FeatureEvent, FeatureFactory};
use crate::models::Instrument;
use crate::state::State;
use parking_lot::Mutex;
use petgraph::graph::NodeIndex;
use petgraph::{
    algo::toposort,
    dot::{Config, Dot},
    graph::DiGraph,
};
use rayon::ThreadPoolBuilder;
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{debug, info};

#[derive(Default)]
pub struct Pipeline {
    state: Arc<State>,
    graph: Arc<DiGraph<Box<dyn Feature>, ()>>,
    order: Vec<NodeIndex>,
}

impl Pipeline {
    pub fn from_config(state: Arc<State>, config: &PipelineConfig) -> Self {
        let mut graph = DiGraph::new();

        // Create features
        let features = FeatureFactory::from_config(&config.features);

        // Add features as nodes
        features.into_iter().for_each(|f| {
            graph.add_node(f);
        });

        // Add edges automatically
        let mut edges_to_add = vec![];
        for target_node in graph.node_indices() {
            for source in graph[target_node].sources() {
                if BASE_IDS.contains(source) {
                    continue;
                }
                let source_node = graph.node_indices().find(|i| graph[*i].id() == source).unwrap();
                edges_to_add.push((source_node, target_node));
            }
        }
        for (source, target) in edges_to_add {
            graph.add_edge(source, target, ());
        }

        // Save down the topological order for parallel processing
        let order = toposort(&graph, None).expect("Cycle detected in graph");

        info!("{:?}", Dot::with_config(&graph, &[Config::EdgeIndexLabel]));
        Pipeline {
            state,
            graph: Arc::new(graph),
            order,
        }
    }

    // Topological Sorting in parallel, which can be efficiently implemented using Kahn's algorithm
    pub fn calculate(&self, instrument: Instrument, event_time: OffsetDateTime) -> Vec<FeatureEvent> {
        // Step 1: Calculate in-degrees
        let in_degrees = Arc::new(Mutex::new(vec![0; self.graph.node_count()]));
        for edge in self.graph.edge_indices() {
            let target = self.graph.edge_endpoints(edge).unwrap().1;
            in_degrees.lock()[target.index()] += 1;
        }
        debug!("In-Degree count: {:?}", in_degrees);

        // Step 2: Enqueue nodes with zero in-degree
        let (queue_tx, queue_rx) = flume::unbounded();
        for node in &self.order {
            if in_degrees.lock()[node.index()] == 0 {
                debug!("Ready node: {:?}", self.graph[*node]);
                queue_tx.send(Some(*node)).expect("Failed to send ready node");
            }
        }

        // Step 3: Parallel processing
        let pipeline_result = Arc::new(Mutex::new(Vec::new()));
        let pool = ThreadPoolBuilder::new().build().expect("Failed to create thread pool");
        pool.scope(|s| {
            while let Some(node) = queue_rx.recv().expect("Failed to receive data") {
                let state = self.state.clone();
                let instrument = instrument.clone();
                let graph = Arc::clone(&self.graph);
                let in_degrees = Arc::clone(&in_degrees);
                let queue_tx = queue_tx.clone();
                let pipeline_result = Arc::clone(&pipeline_result);

                s.spawn(move |_| {
                    // Process the node
                    let feature = &graph[node];

                    // Query the data
                    let data = state.read_features(&instrument, feature.sources(), &event_time, feature.data_type());

                    // Calculate the feature
                    let res = feature.calculate(data);
                    match res {
                        Ok(data) => {
                            debug!("Calculated: {:?}", data);

                            // Save data to state and result set
                            data.into_iter().for_each(|(id, value)| {
                                debug!("Saving: {} => {}", id, value);
                                let event = FeatureEvent::new(id, instrument.to_owned(), event_time, value);
                                state.add_feature(event.clone());
                                pipeline_result.lock().push(event);
                            });
                        }
                        Err(e) => {
                            info!("Failed to calculate: {:?}", e);
                        }
                    }

                    // Update in-degrees of neighbors and enqueue new zero in-degree nodes
                    for neighbor in graph.neighbors_directed(node, petgraph::Outgoing) {
                        let mut in_degrees = in_degrees.lock();
                        in_degrees[neighbor.index()] -= 1;
                        if in_degrees[neighbor.index()] == 0 {
                            debug!("Ready node: {:?}", graph[neighbor]);
                            queue_tx.send(Some(neighbor)).expect("Failed to send ready node");
                        }
                    }
                    debug!("Dependency count: {:?}", in_degrees);
                    if in_degrees.lock().iter().all(|&x| x == 0) {
                        debug!("All nodes processed");
                        queue_tx.send(None).expect("Failed to send exit message");
                    }
                });
            }
        });
        debug!("Finished graph calculation");
        let res = pipeline_result.lock().iter().cloned().collect();
        res
    }

    // COULD BE USED IN THE FUTURE IF WE HAVE ASYNC FEATURES
    // pub async fn calculate_async(&self) {
    //     // Step 1: Calculate in-degrees
    //     let in_degrees = Arc::new(Mutex::new(vec![0; self.graph.node_count()]));
    //     for edge in self.graph.edge_indices() {
    //         let target = self.graph.edge_endpoints(edge).unwrap().1;
    //         in_degrees.lock()[target.index()] += 1;
    //     }
    //     debug!("In-Degree count: {:?}", in_degrees);

    //     // Step 2: Enqueue nodes with zero in-degree
    //     let (queue_tx, queue_rx) = flume::unbounded();
    //     for node in &self.order {
    //         if in_degrees.lock()[node.index()] == 0 {
    //             debug!("Ready node: {:?}", self.graph[*node]);
    //             queue_tx.send(Some(*node)).expect("Failed to send ready node");
    //         }
    //     }

    //     // Step 3: Parallel processing
    //     let mut tasks = Vec::with_capacity(self.graph.node_count());
    //     while let Some(node_index) = queue_rx.recv_async().await.expect("Failed to receive ready node") {
    //         let graph = Arc::clone(&self.graph);
    //         let in_degrees = Arc::clone(&in_degrees);
    //         let queue_tx = queue_tx.clone();

    //         let task = tokio::spawn(async move {
    //             // Calculate the feature
    //             let feature = &graph[node_index];
    //             feature.calculate_async().await;

    //             // Update dependencies and push ready nodes to the queue
    //             for neighbor in graph.neighbors_directed(node_index, petgraph::Outgoing) {
    //                 let mut count = in_degrees.lock()[neighbor.index()];
    //                 count -= 1;
    //                 in_degrees.lock()[neighbor.index()] = count;

    //                 if count == 0 {
    //                     debug!("Ready node: {:?}", graph[neighbor]);
    //                     queue_tx.send_async(Some(neighbor)).await.expect("Failed to send ready node");

    //                     debug!("Dependency count: {:?}", in_degrees);
    //                     if in_degrees.lock().iter().all(|&x| x == 0) {
    //                         queue_tx.send_async(None).await.expect("Failed to send ready node");
    //                     }
    //                 }
    //             }
    //         });
    //         tasks.push(task);
    //     }

    //     // Wait on all tasks to finish
    //     for task in tasks {
    //         let _ = task.await;
    //     }

    //     info!("Finished graph calculation");
    // }
}

// #[derive(Default)]
// struct PipelineState {
//     events: DashMap<(Instrument, FeatureId), BTreeMap<CompositeKey, f64>>,
// }

// impl PipelineState {
//     pub fn insert(&self, event: FeatureEvent) {
//         let key = (event.instrument, event.id);
//         let mut composit_key = CompositeKey::new(&event.event_time);

//         let mut entry = self.events.entry(key).or_default();
//         while entry.get(&composit_key).is_some() {
//             composit_key.increment();
//         }
//         entry.insert(composit_key, event.value);
//     }

//     pub fn query(
//         &self,
//         instrument: &Instrument,
//         feature_ids: Vec<FeatureId>,
//         from: &OffsetDateTime,
//         data_type: QueryType,
//     ) -> HashMap<FeatureId, Vec<f64>> {
//         feature_ids
//             .par_iter()
//             .map(|id| match data_type {
//                 QueryType::Latest => {
//                     let data = self.get_latest(instrument, id, from);
//                     (id.clone(), data)
//                 }
//                 QueryType::Window(window) => {
//                     let data = self.get_range(instrument, id, from, &window);
//                     (id.clone(), data)
//                 }
//                 QueryType::Period(periods) => {
//                     let data = self.get_periods(instrument, id, from, periods);
//                     (id.clone(), data)
//                 }
//             })
//             .collect()
//     }

//     fn get_latest(&self, instrument: &Instrument, feature_id: &FeatureId, from: &OffsetDateTime) -> Vec<f64> {
//         let key = (instrument.clone(), feature_id.clone());
//         let from_key = CompositeKey::new_max(from);

//         if let Some(tree) = self.events.get(&key) {
//             tree.value()
//                 .range(..=from_key)
//                 .rev()
//                 .take(1)
//                 .map(|(k, v)| {
//                     debug!("Found: {} => {}", k, v);
//                     *v
//                 })
//                 .collect()
//         } else {
//             Vec::new()
//         }
//     }

//     fn get_range(
//         &self,
//         instrument: &Instrument,
//         feature_id: &FeatureId,
//         from: &OffsetDateTime,
//         window: &Duration,
//     ) -> Vec<f64> {
//         let key = (instrument.clone(), feature_id.clone());
//         let from_key = CompositeKey::new(from);
//         let end_key = CompositeKey::new(&(*from - *window));

//         debug!("Getting {} for {} from: {} till: {}", feature_id, instrument, from_key, end_key);

//         if let Some(tree) = self.events.get(&key) {
//             tree.value()
//                 .range(end_key..from_key)
//                 .map(|(k, v)| {
//                     debug!("Found: {} => {}", k, v);
//                     *v
//                 })
//                 .collect()
//         } else {
//             Vec::new()
//         }
//     }

//     fn get_periods(
//         &self,
//         instrument: &Instrument,
//         feature_id: &FeatureId,
//         from: &OffsetDateTime,
//         periods: usize,
//     ) -> Vec<f64> {
//         let key = (instrument.clone(), feature_id.clone());
//         let from_key = CompositeKey::new_max(from);

//         if let Some(tree) = self.events.get(&key) {
//             let mut res = tree
//                 .value()
//                 .range(..from_key)
//                 .rev()
//                 .take(periods)
//                 .map(|(k, v)| {
//                     debug!("Found: {} => {}", k, v);
//                     *v
//                 })
//                 .collect::<Vec<_>>();
//             res.reverse();
//             res
//         } else {
//             Vec::new()
//         }
//     }
// }

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::{
//         config::{
//             EMAFeatureConfig, FeatureConfig, SMAFeatureConfig, SpreadFeatureConfig, VWAPFeatureConfig,
//             VolumeFeatureConfig,
//         },
//         logging,
//     };

// #[test]
// fn test_pipeline_add_node() {
//     let mut graph = Pipeline::default();
//     let vwap = VWAPGen::new("vwap".into(), Duration::from_secs(60));
//     graph.add_node(vwap);
//     assert_eq!(graph.graph.node_count(), 1);
// }

// #[test]
// fn test_pipeline_add_edge() {
//     let mut graph = Pipeline::default();
//     let vwap = VWAPGen::new("vwap".into(), Duration::from_secs(60));
//     let ema = EMAGen::new("ema_vwap_50".into(), "vwap".into(), Duration::from_secs(300));

//     graph.add_node(vwap);
//     graph.add_node(ema);

//     graph.connect_nodes();
//     assert_eq!(graph.graph.edge_count(), 1);
// }

// #[tokio::test]
// async fn test_pipeline_calculate() {
//     logging::init_test_tracing();

//     // Create graph
//     let mut graph = Pipeline::default();

//     // Create features
//     let vwap = VWAPGen::new("vwap".into(), Duration::from_secs(60));
//     let ema = EMAGen::new("ema_vwap_50".into(), "vwap".into(), Duration::from_secs(50));
//     let sma = SMAGen::new("sma_vwap_50".into(), "vwap".into(), Duration::from_secs(50));
//     let spread = SpreadGen::new("spread".into(), "sma_vwap_50".into(), "ema_vwap_50".into());
//     let volume = VolumeGen::new("volume".into(), Duration::from_secs(60));

//     // Create nodes
//     graph.add_node(vwap);
//     graph.add_node(ema);
//     graph.add_node(sma);
//     graph.add_node(spread);
//     graph.add_node(volume);

//     // Connect nodes
//     graph.connect_nodes();

//     // Calculate
//     graph.calculate().await;
//     assert_eq!(graph.graph.node_count(), 5);
//     assert_eq!(graph.graph.edge_count(), 4);
// }

// #[tokio::test(flavor = "multi_thread")]
// async fn test_pipeline_from_config() {
//     logging::init_test_tracing();

//     let config = PipelineConfig {
//         name: "test_pipeline".to_string(),
//         frequency: 1,
//         features: vec![
//             FeatureConfig::Volume(VolumeFeatureConfig {
//                 id: "volume_1_min".(),
//                 window: 60,
//             }),
//             FeatureConfig::VWAP(VWAPFeatureConfig {
//                 id: "vwap_1_min".into(),
//                 window: 60,
//             }),
//             FeatureConfig::SMA(SMAFeatureConfig {
//                 id: "sma_10_min".into(),
//                 source: "vwap_1_min".into(),
//                 period: 10,
//             }),
//             FeatureConfig::EMA(EMAFeatureConfig {
//                 id: "ema_10_min".into(),
//                 source: "vwap_1_min".into(),
//                 period: 10,
//             }),
//             FeatureConfig::Spread(SpreadFeatureConfig {
//                 id: "spread_vwap".into(),
//                 front_component: "sma_10_min".into(),
//                 back_component: "ema_10_min".into(),
//             }),
//             FeatureConfig::SMA(SMAFeatureConfig {
//                 id: "sma_volume_10_min".into(),
//                 source: "volume_1_min".into(),
//                 period: 10,
//             }),
//             FeatureConfig::Spread(SpreadFeatureConfig {
//                 id: "spread_volume".into(),
//                 front_component: "sma_volume_10_min".into(),
//                 back_component: "volume_1_min".into(),
//             }),
//             FeatureConfig::Spread(SpreadFeatureConfig {
//                 id: "spread_vwap_volume".into(),
//                 front_component: "spread_vwap".into(),
//                 back_component: "spread_volume".into(),
//             }),
//         ],
//     };

//     let pipeline = Pipeline::from_config(&config);
//     pipeline.calculate();
// }
// }
