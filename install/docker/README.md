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
