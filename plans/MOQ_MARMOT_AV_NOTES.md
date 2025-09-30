# MoQ + Marmot A/V — Notes & Improvements

## SVC Layering for Adaptive Quality

Instead of simulcast (multiple independent encodes), we can use Scalable Video Coding (SVC) with per-layer tracks as described in `draft-lcurley-moq-use-cases.md` §Layers and §SVC.

### Approach

- **Publisher**: Encode with SVC (base + enhancement layers, e.g., 360p base, 1080p enhancement, 4K enhancement)
- **Per-layer tracks**: Each layer gets its own random `label` via MLS exporter and is published as a separate encrypted track under `marmot/<G>/<label>`
- **Directory**: List all layers with metadata linking them (e.g., `"layers": ["360p", "1080p", "4k"]`)
- **Subscriber**: Subscribe to multiple layers with priorities:
  ```
  SUBSCRIBE track=<label_360p>  priority=2 order=DESC
  SUBSCRIBE track=<label_1080p> priority=1 order=DESC
  SUBSCRIBE track=<label_4k>    priority=0 order=DESC
  ```

### Benefits

- **Privacy preserved**: Random labels; relay cannot determine layer relationships without decryption
- **Natural congestion handling**: Relay honors subscription priorities; enhancement layers are deprioritized/dropped first during congestion
- **Efficiency**: ~10% overhead vs simulcast's multiple independent encodes
- **No relay decryption needed**: Priority-based forwarding works on encrypted tracks

### Implementation Notes

- Extend directory schema to include layer metadata (base vs enhancement, dependency chain, resolution/framerate)
- Key derivation per layer: `base = MLS-Exporter("moq-media-base-v1", sender_leaf || track_label || epoch_bytes, 32)`
- Each layer track encrypts independently; subscribers decrypt each layer separately
- Label rotation on epoch change applies per layer

### Trade-offs

- SVC encoder complexity and limited codec support (VP9, AV1 have better support than H.264)
- Small bitrate overhead (~10%) vs non-layered encoding
- Subscribers must handle multi-track subscription and layer reassembly