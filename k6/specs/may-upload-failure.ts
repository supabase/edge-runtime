import http from "k6/http";
import crypto from "k6/crypto";

import { check } from "k6";
import { Options } from "k6/options";

import { target } from "../config";

/** @ts-ignore */
import { randomIntBetween } from "https://jslib.k6.io/k6-utils/1.2.0/index.js"

const MB = 1048576;
const MSG_OP_CANCELED = "Interrupted: operation canceled";

const dummyBinary = crypto.randomBytes(randomIntBetween(25 * MB, 35 * MB));
const dummyFile = http.file(dummyBinary, "dummy", "application/octet-stream");

export const options: Options = {
    stages: [
        {
            duration: "30s",
            target: 3,
        },
        {
            duration: "1m",
            target: 10,
        },
        {
            duration: "1m",
            target: 12
        },
        {
            duration: "30s",
            target: 0
        }
    ]
};

export default function () {
    const res = http.post(
        `${target}/file-upload`,
        {
            "file": dummyFile
        }
    );

    check(res, {
        "status is 200 or 500 (operation canceled)": r => {
            if (r.status === 200) {
                return true;
            }

            if (r.status !== 500) {
                return false;
            }

            let m = r.json();

            if (!m || typeof m !== "object" || !("msg" in m) || m["msg"] !== MSG_OP_CANCELED) {
                return false;
            }

            return true;
        }
    });
}