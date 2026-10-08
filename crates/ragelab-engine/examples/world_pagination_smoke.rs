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
    let mut config = WorldStreamConfig {
        load_radius: 450.0,
        retain_radius: 650.0,
        max_active_maps: 12,
        max_cpu_chunks: 40,
        max_cpu_bytes: 1536 * 1024 * 1024,
        max_chunk_loads_per_update: 2,
        ..WorldStreamConfig::default()
    };
    config.render_options.max_blob_bytes = 256 * 1024 * 1024;
    let mut runtime = WorldStreamingRuntime::open(source, config)?;
    let view = WorldStreamView::at(GtaRpfWorldPoint {
        x: -1475.0,
        y: -850.0,
        z: 10.0,
    });
    let start = Instant::now();
    for tick in 1..=45 {
        let elapsed = Instant::now();
        let update = runtime.update(view)?;
        println!(
            "tick={tick} elapsedMs={:.1} totalMs={:.1} active={} pendingChunks={} pendingPages={} failedPages={} compiledPages={} nodesRemaining={} assetsRemaining={} renderAssetsOmitted={} geometryOmitted={} cpuMiB={:.1} gpuPackagesChanged={}",
            elapsed.elapsed().as_secs_f64() * 1000.0,
            start.elapsed().as_secs_f64() * 1000.0,
            update.report.active_maps,
            update.report.pending_chunks,
            update.report.pending_pages,
            update.report.failed_pages,
            update.report.compiled_pages,
            update.report.spatial_omitted_nodes,
            update.report.spatial_omitted_assets,
            update.report.render_omitted_assets,
            update.report.render_omitted_geometry,
            update.report.cpu_cached_bytes as f64 / (1024.0 * 1024.0),
            update.packages.is_some(),
        );
        for metric in &update.report.chunk_loads {
            println!(
                "  loaded {} ms={:.1}",
                metric.chunk.provider.entry, metric.elapsed_ms
            );
        }
        for error in &update.report.load_errors {
            eprintln!("  ERROR {}: {}", error.chunk.provider.entry, error.message);
        }
        if update.report.pending_chunks == 0 && update.report.pending_pages == 0 {
            break;
        }
    }
    Ok(())
}
