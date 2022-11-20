# centrifugo-change-stream

[![Conventional Commits](https://img.shields.io/badge/Conventional%20Commits-1.0.0-yellow.svg)](https://conventionalcommits.org)

Publish MongoDB changes and proxy subscriptions to Centrifugo.

## Specifications

### MongoDB

This service will establish a connection to MongoDB and watch a [change stream](https://www.mongodb.com/docs/manual/changeStreams/) on the configured database and collection.

It will also query MongoDB for data to send it to the client (as initial data) when a corresponding channel is subscribed.

### Centrifugo

Data from MongoDB change stream will be published to Centrifugo, on the channel with following characteristics:

- [namespace][centrifugo-namespace]: dot-separated MongoDB database and collection (complies with [MongoDB namespace][mongodb-namespace]);
- channel name: primary key of the document (`_id` field).

[centrifugo-namespace]: https://centrifugal.dev/docs/server/channels#channel-namespaces
[mongodb-namespace]: https://www.mongodb.com/docs/manual/reference/glossary/#std-term-namespace

This service will expose a Centrifugo subscribe proxy endpoint on `/centrifugo/subscribe`. For each subscription, it will send initial data in response `data` field.

To check the health of the connection with Centrifugo, this service will publish `null` data to `_` channel.

## Data flow

```mermaid
sequenceDiagram
    participant MongoDB
    participant Me as This service
    participant Centrifugo as Centrifugo server
    participant Client
    critical
        Me->>+MongoDB: Watch for change stream
        MongoDB-->>-Me: Watch established
    end
    par For each client
        Client->>+Centrifugo: Subscribes
        Centrifugo->>+Me: Proxies subscription
        Me->>+MongoDB: queries current data
        MongoDB-->>-Me: Replies with current data
        Me-->>-Centrifugo: Allows subscription, with initial data
        Centrifugo-->>-Client: Allows subscription
        loop Each update from change stream
            MongoDB-)Me: Sends updated document
            activate Me
            Me-)Centrifugo: Publishes the update
            deactivate Me
            activate Centrifugo
            Centrifugo-)Client: Notifies of the update
            deactivate Centrifugo
        end
    end
```

## Usage

```shellSession
$ centrifugo-change-stream --help
Usage: centrifugo-change-stream [OPTIONS] --centrifugo-api-key <CENTRIFUGO_API_KEY> --mongodb-database <MONGODB_DATABASE> --mongodb-collection <MONGODB_COLLECTION>

Options:
  -v, --verbose...
          More output per occurrence
  -q, --quiet...
          Less output per occurrence
      --listen-address <LISTEN_ADDRESS>
          Address to listen on [env: LISTEN_ADDRESS=] [default: 0.0.0.0:8080]
      --centrifugo-api-url <CENTRIFUGO_API_URL>
          Centrifugo server API URL [env: CENTRIFUGO_API_URL=] [default: http://centrifugo:8000/api]
      --centrifugo-api-key <CENTRIFUGO_API_KEY>
          Centrifugo API key [env: CENTRIFUGO_API_KEY=]
      --mongodb-uri <MONGODB_URI>
          URI of MongoDB server [env: MONGODB_URI=] [default: mongodb://mongo]
      --mongodb-database <MONGODB_DATABASE>
          MongoDB database [env: MONGODB_DATABASE=]
      --mongodb-collection <MONGODB_COLLECTION>
          MongoDB collection [env: MONGODB_COLLECTION=]
  -h, --help
          Print help information
```
