import * as net from "ext:deno_net/01_net.js";
import * as tls from "ext:deno_net/02_tls.js";
import * as permissions from "ext:sb_core_main_js/js/permissions.js";
import {
    errors
} from "ext:sb_core_main_js/js/errors.js";
import { serveHttp } from "ext:sb_core_main_js/js/http.js";

const denoOverrides = {
    listen: net.listen,
    connect: net.connect,
    connectTls: tls.connectTls,
    startTls: tls.startTls,
    resolveDns: net.resolveDns,
    serveHttp: serveHttp,
    permissions: permissions.permissions,
    Permissions: permissions.Permissions,
    PermissionStatus: permissions.PermissionStatus,
    errors: errors,
}

export { denoOverrides };