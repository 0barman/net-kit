# net-kit

跨平台网络可达性监听库。对外仅暴露一个 `Net` 结构体，内部基于精简版
[`vibe-ready`](https://crates.io/crates/vibe-ready) 运行时驱动 [`netwatch`] 网络监听。

## 特性

- 单一对外 API 结构体 `Net`，所有内部逻辑隔离在 `inner` 模块。
- 支持两种运行时启动方式：
  - `start()`：使用库自带的运行时引擎。
  - `start_with_tokio_rt(handle)`：复用开发者自己的 Tokio 运行时（该运行时由调用方保活，`shutdown` 不会关闭它）。
- 注册多个监听回调，回调在引擎回调线程池上触发，不阻塞监听任务。
- Windows 下基于 `INetworkListManager` 判定 Internet 可达性。

## 用法

```rust
use std::time::Duration;
use net_utils::{Net, NetworkStatus};

#[tokio::main]
async fn main() {
    let net = Net::new();

    // 使用库自带运行时启动监听。
    net.start().await.expect("start failed");

    // 所有触碰内部状态的方法都返回 Result，错误（如内部锁损坏）会显式交还给
    // 开发者处理，库自身不会 panic。
    let reachability = net
        .local_network_reachability()
        .expect("query reachability failed");
    println!("reachability: {reachability:?}");

    // 注册网络状态变化监听（可注册多个）。未启动时返回 Ok(None)。
    let handle = net
        .register(Box::new(|status: NetworkStatus| {
            println!("network status changed: {status:?}");
        }))
        .expect("register failed")
        .expect("register requires start");

    tokio::time::sleep(Duration::from_secs(5)).await;

    net.unregister(handle).expect("unregister failed");
    net.shutdown().expect("shutdown failed");
}
```

完整的可运行示例见 [`demo`](../demo) crate，可通过 `cargo run -p demo` 运行（演示启动/查询/监听回调中打印当前网络名称/关闭的完整流程）。

## 生命周期

- `start` / `start_with_tokio_rt`：重复调用会被忽略；可在 `shutdown` 后再次调用以重建。
- `shutdown`：停止监听任务并销毁引擎，释放所有 `start` 创建的资源；重复调用会被忽略。

[`netwatch`]: https://crates.io/crates/netwatch
