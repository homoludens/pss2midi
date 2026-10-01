./target/release/pss2midi \
  --device pipewire \
  --hop 128 \
  --pitch-buffer 2048 \
  --onset-buffer 1024 \
  --silence-db=-45 \
  --attack-ignore-ms 10 \
  --decision-window-ms 20 \
  --release-ms 30 \
  --retrigger-ms 90 \
  --debug

# For tuning, first use:
./target/release/pss2midi \
  --device pipewire \
  --debug


  You should see things such as:
  ONSET level=-11.8 dB pitch=64.21
  attack-ignore pitch=40.43
  vote C3 (+4.2c)
  vote C3 (+2.6c)
  vote C3 (+1.1c)
  decision C3 votes=5 ratio=0.83
  
  ON   C3   MIDI=48
