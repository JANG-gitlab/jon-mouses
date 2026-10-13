#!/bin/sh
# Keeps AWDL (AirDrop, Continuity) off while Jon Mouse's runs: its channel
# hops stall Wi-Fi for 50 to 100 ms every second, which shows as cursor jumps.
restore=0
while :; do
	if /usr/bin/pgrep -qx jon-mouses; then
		/sbin/ifconfig awdl0 | /usr/bin/grep -q '<UP' && /sbin/ifconfig awdl0 down
		restore=1
	elif [ "$restore" = 1 ]; then
		/sbin/ifconfig awdl0 up
		restore=0
	fi
	/bin/sleep 1
done
