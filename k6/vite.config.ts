// copied from https://github.com/Jahmilli/k6-example

import { babel } from "@rollup/plugin-babel";
import { nodeResolve } from "@rollup/plugin-node-resolve";
import { defineConfig } from "vite";

import copy from "rollup-plugin-copy";
import fg from "fast-glob";

const getEntryPoints = (entryPoints) => {
    const files = fg.sync(entryPoints, { absolute: true });

    const entities = files.map((file) => {
        const [key] = file.match(/(?<=k6\/).*$/) || [];
        const keyWithoutExt = key?.replace(/\.[^.]*$/, "");

        return [keyWithoutExt, file];
    });

    return Object.fromEntries(entities);
};

export default defineConfig({
    mode: "production",
    build: {
        lib: {
            entry: getEntryPoints(["./specs/*.ts"]),
            fileName: "[name]",
            formats: ["cjs"],
        },
        outDir: "dist",
        minify: false,
        rollupOptions: {
            external: [new RegExp(/^(k6|https?\:\/\/)(\/.*)?/)],
        },
    },
    resolve: {
        extensions: [".ts", ".js"],
    },
    plugins: [
        copy({
            targets: [
                {
                    src: "assets/**/*",
                    dest: "dist",
                },
            ],
        }),
        babel({
            babelHelpers: "bundled",
            exclude: /node_modules/,
        }),
        nodeResolve(),
    ],
});