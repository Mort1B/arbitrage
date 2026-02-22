use crate::SignalLogSender;
use log::{error, info};
use tokio::{
    fs::{create_dir_all, OpenOptions},
    io::{AsyncWriteExt, BufWriter},
    sync::mpsc,
};

pub fn spawn_jsonl_writer(path: String, capacity: usize) -> SignalLogSender {
    let (tx, mut rx) = mpsc::channel::<String>(capacity);

    tokio::spawn(async move {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = create_dir_all(parent).await {
                    error!("failed to create signal log directory {:?}: {}", parent, e);
                    while rx.recv().await.is_some() {}
                    return;
                }
            }
        }

        let file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
        {
            Ok(file) => file,
            Err(e) => {
                error!("failed to open signal log file {}: {}", path, e);
                while rx.recv().await.is_some() {}
                return;
            }
        };

        info!("writing triangle signals to {}", path);
        let mut writer = BufWriter::new(file);
        let mut pending_flush = 0usize;

        while let Some(line) = rx.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err()
                || writer.write_all(b"\n").await.is_err()
            {
                error!("failed to write to signal log file {}", path);
                break;
            }

            pending_flush += 1;
            if pending_flush >= 32 {
                if let Err(e) = writer.flush().await {
                    error!("failed to flush signal log file {}: {}", path, e);
                    break;
                }
                pending_flush = 0;
            }
        }

        if let Err(e) = writer.flush().await {
            error!(
                "failed to flush signal log file on shutdown {}: {}",
                path, e
            );
        }
    });

    tx
}
