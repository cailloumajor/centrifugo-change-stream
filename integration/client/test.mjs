import assert from "node:assert";
import { basename } from "node:path";
import { argv } from "node:process";

import { Centrifuge } from "centrifuge";
import WebSocket from "ws";

const assertInRange = (n, min, max) =>
    assert.ok(n >= min && n <= max, `unexpected ${min} <= ${n} <= ${max}`);
const assertIntegerRange = (n) => assertInRange(n, 150, 160);
const assertFloatRange = (n) => assertInRange(n, 32.0, 35.0);
const assertDate = (ts) =>
    assert.ok(!isNaN(Date.parse(ts)), `unexpected timestamp: ${ts}`);

let noDataSubscribed = false;
const publications = [];

const centrifuge = new Centrifuge("ws://centrifugo:8000/connection/websocket", {
    debug: true,
    websocket: WebSocket,
});

const noDataSub = centrifuge.newSubscription("testdb.testcoll:nodata");

noDataSub.on("subscribed", ({ channel, data }) => {
    console.log(
        "got subscribed event for `%s` channel, with data: %O",
        channel,
        data
    );
    noDataSubscribed = true;
    console.log("testing no-data channel data object");
    assert.strictEqual(Object.keys(data).length, 0);
    assert.strictEqual(data.constructor, Object);
});

noDataSub.on("publication", () => {
    throw new Error("unexpected publication on no-data channel");
});

noDataSub.subscribe();

const sub = centrifuge.newSubscription("testdb.testcoll:integration-tests");

sub.on("subscribed", ({ channel, data }) => {
    console.log(
        "got subscribed event for `%s` channel, with data: %O",
        channel,
        data
    );
    console.log("testing data object validity from subscribe proxy");
    assertIntegerRange(data.val.integer);
    assertFloatRange(data.val.float);
    assertDate(data.ts.first);
    assertDate(data.ts.second);
});

sub.on("publication", ({ data }) => {
    console.log("got publication with data: %O", data);
    publications.push(data);
});

sub.subscribe();

centrifuge.connect();

const publicationsTimeout = setTimeout(() => {
    throw new Error("timeout waiting for publications array to populate");
}, 10000);

await new Promise((resolve) => {
    const interval = setInterval(() => {
        if (publications.length >= 6) {
            clearInterval(interval);
            resolve();
        }
    }, 500);
});

centrifuge.disconnect();
clearTimeout(publicationsTimeout);

assert.ok(noDataSubscribed, "no-data channel has not been subscribed");

console.log("testing validity of received publications");
const integerPublished = publications.filter(
    (data) => data.val?.integer != null
);
assert.strictEqual(integerPublished.length, 4);
integerPublished.forEach((data) => assertIntegerRange(data.val.integer));
const floatPublished = publications.filter((data) => data.val?.float != null);
assert.strictEqual(floatPublished.length, 4);
floatPublished.forEach((data) => assertFloatRange(data.val.float));
const tsFirstPublished = publications.filter((data) => data.ts?.first != null);
assert.strictEqual(tsFirstPublished.length, 4);
tsFirstPublished.forEach((data) => assertDate(data.ts.first));
const tsSecondPublished = publications.filter(
    (data) => data.ts?.second != null
);
assert.strictEqual(tsSecondPublished.length, 4);
tsSecondPublished.forEach((data) => assertDate(data.ts.second));

console.log("%s: success", basename(argv[1]));
