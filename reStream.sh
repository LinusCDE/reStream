#!/bin/sh

# default values for arguments
ssh_host="root@10.11.99.1" # remarkable connected through USB
landscape=true             # rotate 90 degrees to the right
output_path=-              # display output through ffplay
format=-                   # automatic output format
webcam=false               # not to a webcam
measure_throughput=false   # measure how fast data is being transferred
window_title=reStream      # stream window title is reStream
video_filters=""           # list of ffmpeg filters to apply
unsecure_connection=false  # Establish a unsecure connection that is faster
xor=false                  # Use xor compression to make lz4 compression more effective

# loop through arguments and process them
while [ $# -gt 0 ]; do
    case "$1" in
        -p | --portrait)
            landscape=false
            shift
            ;;
        -s | --source)
            ssh_host="$2"
            shift
            shift
            ;;
        -o | --output)
            output_path="$2"
            shift
            shift
            ;;
        -f | --format)
            format="$2"
            shift
            shift
            ;;
        -m | --measure)
            measure_throughput=true
            shift
            ;;
        -w | --webcam)
            webcam=true
            format="v4l2"
            # check if there is a modprobed v4l2 loopback device
            # use the first cam as default if there is no output_path already
            cam_path=$(v4l2-ctl --list-devices \
                | sed -n '/^[^\s]\+platform:v4l2loopback/{n;s/\s*//g;p;q}')

            # fail if there is no such device
            if [ -e "$cam_path" ]; then
                if [ "$output_path" = "-" ]; then
                    output_path="$cam_path"
                fi
            else
                echo "Could not find a video loopback device, did you"
                echo "sudo modprobe v4l2loopback"
                exit 1
            fi
            shift
            ;;
        -t | --title)
            window_title="$2"
            shift
            shift
            ;;
        -u | --unsecure-connection)
            unsecure_connection=true
            shift
            ;;
        -x | --xor)
            xor=true
            shift
            ;;
        -h | --help | *)
            echo "Usage: $0 [-p] [-u] [-x] [-s <source>] [-o <output>] [-f <format>] [-t <title>]"
            echo "Examples:"
            echo "	$0                              # live view in landscape"
            echo "	$0 -p                           # live view in portrait"
            echo "	$0 -s 192.168.0.10              # connect to different IP"
            echo "	$0 -o remarkable.mp4            # record to a file"
            echo "	$0 -o udp://dest:1234 -f mpegts # record to a stream"
            echo "  $0 -w                           # write to a webcam (yuv420p + resize)"
            echo "  $0 -u                           # establish a unsecure but faster connection"
            echo "  $0 -x                           # xor frames to increase effectiveness of compression"
            exit 1
            ;;
    esac
done

ssh_cmd() {
    echo "[SSH]" "$@" >&2
    ssh -o ConnectTimeout=1 -o PasswordAuthentication=no "$ssh_host" "$@"
}

# check if we are able to reach the remarkable
if ! ssh_cmd true; then
    echo "$ssh_host unreachable or you have not set up an ssh key."
    echo "If you see a 'Permission denied' error, please visit"
    echo "https://github.com/rien/reStream/#installation for instructions."
    exit 1
fi

rm_version="$(ssh_cmd cat /sys/devices/soc0/machine)"

case "$rm_version" in
    "reMarkable 1.0")
        width=1408
        height=1872
        pixel_format="rgb565le"
        frame_size=$((width * height * 2))
        ;;
    "reMarkable 2.0")
        pixel_format="gray8"
        width=1872
        height=1404
        frame_size=$((width * height * 1))
        video_filters="$video_filters,transpose=2"
        ;;
    *)
        echo "Unsupported reMarkable version: $rm_version."
        echo "Please visit https://github.com/rien/reStream/ for updates."
        exit 1
        ;;
esac

# technical parameters
loglevel="info"
decompress="lz4 -d"

# check if lz4 is present on the host
if ! lz4 -V >/dev/null; then
    echo "Your host does not have lz4."
    echo "Please install it using the instruction in the README:"
    echo "https://github.com/rien/reStream/#installation"
    exit 1
fi

# check if restream binay is present on remarkable
if ssh_cmd "[ ! -f ~/restream ]"; then
    echo "The restream binary is not installed on your reMarkable."
    echo "Please install it using the instruction in the README:"
    echo "https://github.com/rien/reStream/#installation"
    exit 1
fi

# use pv to measure throughput if desired, else we just pipe through cat
if $measure_throughput; then
    if ! pv --version >/dev/null; then
        echo "You need to install pv to measure data throughput."
        exit 1
    else
        loglevel="error" # verbose ffmpeg output interferes with pv
        host_passthrough="pv"
    fi
else
    host_passthrough="cat"
fi

# store extra ffmpeg arguments in $@
set --

# rotate 90 degrees if landscape=true
$landscape && video_filters="$video_filters,transpose=1"

# Scale and add padding if we are targeting a webcam because a lot of services
# expect a size of exactly 1280x720 (tested in Firefox, MS Teams, and Skype for
# for business). Send a PR if you can get a higher resolution working.
if $webcam; then
    video_filters="$video_filters,format=pix_fmts=yuv420p"
    video_filters="$video_filters,scale=-1:720"
    video_filters="$video_filters,pad=1280:0:-1:0:#eeeeee"
fi

# set each frame presentation time to the time it is received
video_filters="$video_filters,setpts=(RTCTIME - RTCSTART) / (TB * 1000000)"

set -- "$@" -vf "${video_filters#,}"

if [ "$output_path" = - ]; then
    output_cmd=ffplay

    window_title_option="-window_title $window_title"
else
    output_cmd=ffmpeg

    if [ "$format" != - ]; then
        set -- "$@" -f "$format"
    fi

    set -- "$@" "$output_path"
fi

set -e # stop if an error occurs

# Tell restream to use xor if selected by user
restream_opts=""
unxor_passthrough="cat"
if $xor; then
  restream_opts="$restream_opts --xor"
  #unxor="target/release/unxor"
  unxor_passthrough="./unxor $frame_size"
  # TODO: Use .exe for windows?
fi

receive_cmd="ssh_cmd ./restream $restream_opts"

if $unsecure_connection; then
  echo "Spawning unsecure connection"
  ssh_cmd 'sleep 0.25 && ./restream '"$restream_opts"' --connect "$(echo $SSH_CLIENT | cut -d " " -f1):61819"' &
  receive_cmd="nc -l -p 61819"
fi

# shellcheck disable=SC2086
$receive_cmd \
    | $decompress \
    | $unxor_passthrough \
    | $host_passthrough \
    | "$output_cmd" \
        -vcodec rawvideo \
        -loglevel "$loglevel" \
        -f rawvideo \
        -pixel_format "$pixel_format" \
        -video_size "$width,$height" \
        $window_title_option \
        -i - \
        "$@"
