I'm trying Axum for a little project, and ran into an odd concurrency behavior that I don't understand. Close-to-minimal example: https://github.com/cceckman/slow-axum

The routing I've set up:

- The root router serves HTML, with four images on it; in order, `/stateless/1.png`, `/stateless/2.png`, `/stateful/1.png`, `/stateful/2.png`.
- The `stateless` route takes a `Path` argument; the `stateful` route takes a `State` argument. Neither argument actually does anything.
- All the `.png` routes perform a blocking sleep[^1] for 5 seconds then return an image.

`main.rs` sets up a Tokio multi-threaded executor and starts Axum serving on port 3000.

When I run this (`nproc` is 12) and load the page, my browser's debug tools indicate:

- `/` completes quickly
- The first image load (`/stateless/1.png`) takes 5 seconds
- All other image loads (`/stateless/2.png` and both `/stateful/`) take 10 seconds, request-to-response

It seems like the first image load is blocking all the others from starting, but then all the others proceed concurrently.

Traces from `tower_http` indicate that the first request is completed before the remaining ones are served:

```
2024-02-18T18:08:52.481171Z DEBUG http_request{method=GET query="/"}: tower_http::trace::on_request: started processing request
2024-02-18T18:08:52.481360Z DEBUG http_request{method=GET query="/"}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
2024-02-18T18:08:52.504262Z DEBUG http_request{method=GET query="/stateless/1.png"}: tower_http::trace::on_request: started processing request
2024-02-18T18:08:57.504606Z DEBUG http_request{method=GET query="/stateless/1.png"}: tower_http::trace::on_response: finished processing request latency=5000 ms status=200
2024-02-18T18:08:57.505527Z DEBUG http_request{method=GET query="/stateless/2.png"}: tower_http::trace::on_request: started processing request
2024-02-18T18:08:57.505735Z DEBUG http_request{method=GET query="/stateful/2.png"}: tower_http::trace::on_request: started processing request
2024-02-18T18:08:57.505739Z DEBUG http_request{method=GET query="/favicon.ico"}: tower_http::trace::on_request: started processing request
2024-02-18T18:08:57.505859Z DEBUG http_request{method=GET query="/stateful/1.png"}: tower_http::trace::on_request: started processing request
2024-02-18T18:08:57.505904Z DEBUG http_request{method=GET query="/favicon.ico"}: tower_http::trace::on_response: finished processing request latency=0 ms status=404
2024-02-18T18:09:02.505869Z DEBUG http_request{method=GET query="/stateless/2.png"}: tower_http::trace::on_response: finished processing request latency=5000 ms status=200
2024-02-18T18:09:02.506060Z DEBUG http_request{method=GET query="/stateful/2.png"}: tower_http::trace::on_response: finished processing request latency=5000 ms status=200
2024-02-18T18:09:02.506158Z DEBUG http_request{method=GET query="/stateful/1.png"}: tower_http::trace::on_response: finished processing request latency=5000 ms status=200
```

What's going on here? Why are the other images serialized after the first one completes?


[^1]: Yes, I know that we don't actually want to do a blocking sleep in an `async` context. This is simulating CPU-heavy work, and I'm eventually planning on offloading that with `spawn_blocking` - but I first want to understand why it's blocking the other requests in this contxt.

## Prior questions

-   ["Why blocking in the same routers?"](https://github.com/tokio-rs/axum/discussions/2321) - appears to be a misunderstanding of the behavior

-   ["tokio::spawn makes axum won't take request in parallel"](https://github.com/tokio-rs/axum/discussions/1695)
-   ["tokio::time::sleep blocks the request thread"](https://github.com/tokio-rs/axum/discussions/2436) - indicates it's client-side.

These two, collectively, hint at a reason...

In all of my cases, it's eventually degrading to HTTP/1.1- which explicitly treats requests as serial.

If Axum has a _slightly_ suboptimal behavior, we might run into something like this.

## Hypothesis

Let's think about the order of events for a moment, starting from when the client starts sending image requests.

1.  Client enqueues four requests, serially.
2.  Axum reads from a TCP connection - sees the first request.
3.  Axum walks its routers, generates a `Future` for this request... and immediately polls on it.

    If this is from the same thread that is reading the HTTP session - uh oh! We've stalled out future requests.

4.  Axum completes processing of the first request, and goes back to reading the HTTP session.
    It reads _all available_ requests, and dispatches them as separate futures...

But why would Axum have a different conccurrency behavior in (4)?

I'd need to find the poll-for-reads point to see...or trace what workers the various things are happening in.

## Experiment

Using `spawn` and `await` in all workers doesn't help.

Using `spawn_blocking` _does_ - reduces the overall latency to just 5s, all requests start blocking before any are serviced.

Running `curl` for the same URL, multiple connections at a time - they proceed in parallel.

---

So... the _apparent_ behavior of Axum is: requests are read and dispatched in batches, per-connection? At least with HTTP/1.1.
