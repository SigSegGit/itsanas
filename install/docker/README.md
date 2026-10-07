# The coordinator in Docker

The same coordinator as `install/coordinator.sh`, in a container whose memory,
CPU and process count are capped, so a flood, a bug or a crowd of members slows
the coordinator and nothing else on the machine.

| Limit | Default | Change with |
| --- | --- | --- |
| memory (no swap) | 256 MB | `ITSANAS_COORD_MEMORY=512m` |
| CPU | half a core | `ITSANAS_COORD_CPUS=1` |
| processes / threads | 256 | `pids_limit` in `compose.yml` |

The root filesystem is read-only, every capability is dropped, and the image
has no shell. Its state (device id, members) is `/var/lib/itsanas-coordinator`,
the directory the systemd install used, so moving to Docker keeps the device id
members pin.

## From the systemd install to Docker

```sh
cd ~/itsanas && git pull --ff-only
sudo systemctl disable --now itsanas-coordinator itsanas-coordinator-update.timer
sudo chown -R 65532:65532 /var/lib/itsanas-coordinator
docker compose -f install/docker/compose.yml up -d --build
docker logs itsanas-coordinator          # same device id as before
```

## Watching it

- `docker stats itsanas-coordinator`: its CPU, memory against its limit, network.
- the machine as a whole: `htop`, or your usual monitoring.
- `docker logs -f itsanas-coordinator`: what it says.

## Updating it

`sh install/docker/update.sh` pulls `main` (fast-forward only) and rebuilds when
it moved. Each night, as the checkout's owner (who must be in the `docker`
group): `crontab -e`, then
`0 4 * * * sh $HOME/itsanas/install/docker/update.sh >> $HOME/itsanas-update.log 2>&1`.

## A client in Docker (any Linux, the Raspberry Pi included)

`compose.client.yml` runs one member node, apart from the coordinator. It uses
the host's network (local discovery and the router's port need it), runs as
your user so the synced files are yours, and is capped at 512 MB and one core
(`ITSANAS_CLIENT_MEMORY`, `ITSANAS_CLIENT_CPUS`). One project (`-p NAME`) per
account; a second account on the machine is a second project with its own data
directory, folder and listen port.

```sh
cd ~/itsanas/install/docker
mkdir -p data ITSaNAS
cp client.env.example client.env && chmod 600 client.env && nano client.env   # the passphrase
C="docker compose -f compose.client.yml -p itsanas-nicolas"
$C build
$C run --rm client init --username nicolas        # first machine: shows the 24 words
#   or: $C run --rm -it client login --username nicolas   (another machine of an existing account)
$C run --rm client folder /data/folder
$C run --rm client coordinator itsanas.ngas.fr:9898 --device 2cfb515fc90749d7248f4af404ecb34b5417e0c1339565c8036a9dde70cf72ce
$C run --rm client register
$C up -d
$C logs -f client
```

The synced folder is `install/docker/ITSaNAS` (or `ITSANAS_FOLDER=/path`).
Updating: `git pull && $C up -d --build`. `itsanas update` does not apply in a
container: the image is rebuilt instead.
