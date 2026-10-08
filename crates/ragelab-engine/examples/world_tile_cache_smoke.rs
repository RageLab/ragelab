use std::{error::Error, path::PathBuf, time::Instant};

use ragelab_engine::{
    GtaRpfWorldPoint, SceneGameIndexSource, WorldStreamConfig, WorldStreamView,
    WorldStreamingRuntime,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let source = SceneGameIndexSource::new(
        PathBuf::from(args.next().ok_or("missing GTA Legacy root")?),
        PathBuf::from(args.next().ok_or("missing GTA asset index")?),
        PathBuf::from(args.next().ok_or("missing GTA key store")?),
    );

    let config = WorldStreamConfig {
        load_radius: 450.0,
        retain_radius: 650.0,
        max_active_maps: 12,
        max_cpu_chunks: 40,
        max_cpu_bytes: 1536 * 1024 * 1024,
        max_chunk_loads_per_update: 2,
        ..WorldStreamConfig::default()
    };

    let mut runtime = WorldStreamingRuntime::open(source, config)?;
    for (name, x, y) in [
        ("spawn-1", -1475.0, -850.0),
        ("spawn-2", -1475.0, -850.0),
        ("spawn-3", -1475.0, -850.0),
        ("spawn-4", -1475.0, -850.0),
        ("spawn-5", -1475.0, -850.0),
        ("spawn-6", -1475.0, -850.0),
        ("same-cell", -1474.6, -850.0),
        ("next-cell-1", -1340.0, -850.0),
        ("next-cell-2", -1340.0, -850.0),
        ("next-cell-3", -1340.0, -850.0),
        ("return-1", -1475.0, -850.0),
        ("return-2", -1475.0, -850.0),
    ] {
        let started = Instant::now();
        let view = WorldStreamView::at(GtaRpfWorldPoint { x, y, z: 10.0 });
        let update = runtime.update(view)?;
        println!(
            "{} elapsedMs={:.1} active={} pending={} loaded={} cacheHits={} cacheMisses={} tileSwaps={} retiredTiles={} omittedNodes={} omittedAssets={}",
            name,
            started.elapsed().as_secs_f64() * 1000.0,
            update.report.active_maps,
            update.report.pending_chunks,
            update.report.loaded_chunks,
            update.report.cache_hits,
            update.report.cache_misses,
            update.report.cached_tile_swaps,
            update.report.cached_retired_tiles,
            update.report.spatial_omitted_nodes,
            update.report.spatial_omitted_assets,
        );
        for metric in &update.report.chunk_loads {
            println!(
                "  {} prefetch={} elapsedMs={:.1}",
                metric.chunk.provider.entry, metric.prefetched, metric.elapsed_ms
            );
        }
    }
    Ok(())
}
