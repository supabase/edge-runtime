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

export default function simple() {
    const res = http.get(`${target}/serve`);

    check(res, {
        "status is 200": r => r.status === 200
    });
}