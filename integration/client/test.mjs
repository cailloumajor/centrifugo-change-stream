import { basename } from "node:path";
import { argv } from "node:process";
import { format } from "node:util";

import { Centrifuge } from "centrifuge";
import WebSocket from "ws";

const inRange = (n, min, max) => n >= min && n <= max;
const validDate = (ts) => !isNaN(Date.parse(ts));

const checkData = (data, context) => {
    const errorMessage = (member) =>
        format(
            "testing %s failed for %s member, element: %s",
            context,
            data,
            member
        );

    if (data.integer && !inRange(data.integer, 150, 250)) {
        throw new Error(errorMessage("integer"));
    }
    if (data.float && !inRange(data.float, 32.0, 42.0)) {
        throw new Error(errorMessage("float"));
    }
    if (data.ts?.first && !validDate(data.ts.first)) {
        throw new Error(errorMessage("ts.first"));
    }
    if (data.ts?.second && !validDate(data.ts.second)) {
        throw new Error(errorMessage("ts.second"));
    }
};

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
    if (Object.keys(data).length !== 0 || data.constructor !== Object) {
        throw new Error("no-data channel: expected empty data object");
    }
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
    if (!data.integer || !data.float || !data.ts?.first || !data.ts?.second) {
        throw new Error("missing data property(ies) in initial data");
    }
    checkData(data, "subscribe proxy");
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
        let membersCount = [0, 0, 0, 0];
        publications.forEach((pub) => {
            if (pub.integer) {
                membersCount[0] += 1;
            }
            if (pub.float) {
                membersCount[1] += 1;
            }
            if (pub.ts?.first) {
                membersCount[2] += 1;
            }
            if (pub.ts?.second) {
                membersCount[3] += 1;
            }
        });
        if (!membersCount.some((count) => count < 5)) {
            clearInterval(interval);
            resolve();
        }
    }, 500);
});

centrifuge.disconnect();
clearTimeout(publicationsTimeout);

if (!noDataSubscribed) {
    throw new Error("no-data channel has not been subscribed");
}

publications.forEach((publication) => checkData(publication, "publication"));

console.log("%s: success", basename(argv[1]));
