/*
./scripts/run.sh

#!/usr/bin/env bash

GIT_V_TAG=0.1.1 cargo build --features cli/tracing && \
EDGE_RUNTIME_WORKER_POOL_SIZE=8 \
EDGE_RUNTIME_PORT=9998 RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start \
    --main-service ./examples/main \
    --event-worker ./examples/event-manager

*/

import http from "k6/http";

import { check } from "k6";
import { Options } from "k6/options";

import { target } from "../config";

export const options: Options = {
    scenarios: {
        simple: {
            executor: "constant-vus",
            vus: 12,
            duration: "3m",
        }
    }
};

export default function gte() {
    const res = http.post(
        `${target}/k6-gte`,
        JSON.stringify({
            "text_for_embedding": "meow"
        })
    );

    check(res, {
        "status is 200": r => r.status === 200
    });
}