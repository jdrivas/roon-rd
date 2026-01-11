# Roon Remote Display - TODO List

## Future Investigations and Improvements

### Roon API Event Handling
1. **Make sure we log all `Parsed::Error` events as INFO in the server**
   - Currently these errors might not be logged properly
   - Need to add explicit logging for API errors

2. **Find out what `Parsed::Outputs(Vec<Output>)` means**
   - This event is currently not handled in our code
   - Need to understand when it's sent and what data it contains
   - Determine if we should react to it

3. **✅ Handle `Parsed::Queue(Vec<QueueItem>)` and `Parsed::QueueChanges(Vec<QueueChange>)`**
   - ✅ Implemented on-demand queue subscription (Option C)
   - ✅ Queue events are properly handled and stored
   - ✅ Queue changes (insert/remove) are applied to cached queue

4. **Determine what the `Parsed::SettingsSubscribed/Unsubscribed/Saved` events and the Settings feature in general is**
   - Settings feature is currently not enabled in our Cargo.toml dependencies
   - Need to understand what Roon settings this exposes
   - Determine if we want to enable this feature and how to use it

## Performance and UI Issues

### High Priority - Fix Zone Re-rendering
- **Problem**: `Parsed::ZonesSeek` (sent every ~1 second during playback) triggers full zone re-render
- **Symptoms**:
  - Queue button flashing when hovering
  - Queue popup disappearing after double-click
  - Poor performance with full DOM rebuild every second
- **Solution**: Add separate WebSocket message type for seek updates that only updates progress bars
  - Consider comparing old vs new data before updating
  - Use DOM manipulation instead of `innerHTML =`

## Multi-Zone Queue Management - Alternate Approaches

### Current Implementation: Option C - On-Demand Subscription (✅ Implemented)
**What it does:**
- Single active queue subscription at a time
- Subscribes to zone's queue when user clicks queue button
- Automatically unsubscribes from previous zone before subscribing to new one
- Tracks active zone via `active_queue_zone: Arc<RwLock<Option<String>>>`

**Advantages:**
- ✅ Simple to implement within existing rust-roon-api constraints
- ✅ Matches user interaction pattern (viewing one queue at a time)
- ✅ Reduces WebSocket traffic and server load
- ✅ No library modifications needed
- ✅ Works reliably with REQUEST/CONTINUE correlation

**Disadvantages:**
- ❌ Queue data for inactive zones becomes stale
- ❌ Must re-subscribe when switching back to a zone
- ❌ Brief delay when opening queue (waiting for subscription)

---

### Option A - Raw Message Interception (Not Implemented)
**How it would work:**
- Intercept WebSocket messages before rust-roon-api parsing
- Parse queue events ourselves for ALL request_ids
- Track multiple `request_id -> zone_id` mappings
- Bypass rust-roon-api's single-subscription filtering

**Implementation approach:**
```rust
// Track multiple queue subscriptions
queue_subscriptions: Arc<RwLock<HashMap<String, (usize, usize)>>> // zone_id -> (req_id, sub_key)

// Intercept raw messages in event loop before parse_msg()
if msg["name"] == "Subscribed" || msg["name"] == "Changed" {
    let req_id = msg["request_id"];
    // Find zone_id by looking up req_id in our tracking map
    // Parse and store queue data ourselves
}
```

**Advantages:**
- ✅ All zone queues stay current simultaneously
- ✅ No delay when switching queue views
- ✅ Complete queue history maintained

**Disadvantages:**
- ❌ Duplicates rust-roon-api parsing logic
- ❌ Requires deep understanding of Roon WebSocket protocol
- ❌ Must maintain synchronization with library updates
- ❌ Complex error handling and edge cases
- ❌ Higher memory usage (storing multiple queues)
- ❌ Increased WebSocket traffic

**When to consider:**
- Application needs to display multiple zone queues simultaneously
- Real-time queue monitoring across all zones is critical
- Willing to maintain custom protocol parsing code

---

### Option B - Modify rust-roon-api Locally (Not Implemented)
**How it would work:**
- Fork/vendor rust-roon-api library (already done)
- Modify Transport struct to support multiple queue subscriptions
- Change storage from single to HashMap-based

**Implementation changes needed in rust-roon-api:**

1. **In transport.rs - struct Transport:**
```rust
// Change from:
queue_sub: Arc<Mutex<Option<(usize, usize)>>>

// To:
queue_subs: Arc<Mutex<HashMap<String, (usize, usize)>>> // zone_id -> (req_id, sub_key)
```

2. **In transport.rs - subscribe_queue():**
```rust
pub async fn subscribe_queue(&self, zone_or_output_id: &str, max_item_count: u32) {
    if let Some(moo) = &self.moo {
        let args = json!({
            "zone_or_output_id": zone_or_output_id,
            "max_item_count": max_item_count,
        });

        let sub = moo.send_sub_req(SVCNAME, "queue", Some(args)).await.ok();

        // Store subscription per zone instead of overwriting
        if let Some(sub) = sub {
            self.queue_subs.lock().await.insert(zone_or_output_id.to_string(), sub);
        }
    }
}
```

3. **In transport.rs - parse_msg():**
```rust
// Change from checking single queue_req_id:
if let Some((queue_req_id, _)) = *self.queue_sub.lock().await {
    if req_id == queue_req_id {
        // Parse queue events
    }
}

// To checking all tracked request_ids:
let queue_subs = self.queue_subs.lock().await;
for (zone_id, (queue_req_id, _)) in queue_subs.iter() {
    if req_id == *queue_req_id {
        // Parse queue events and include zone_id in Parsed event
        // Would need to change Parsed::Queue and Parsed::QueueChanges to include zone_id
        break;
    }
}
```

4. **In lib.rs - Parsed enum:**
```rust
// Change from:
Parsed::Queue(Vec<QueueItem>)
Parsed::QueueChanges(Vec<QueueChange>)

// To:
Parsed::Queue { zone_id: String, items: Vec<QueueItem> }
Parsed::QueueChanges { zone_id: String, changes: Vec<QueueChange> }
```

**Advantages:**
- ✅ Clean integration with existing code patterns
- ✅ All zone queues stay current
- ✅ Events include zone_id for easy correlation
- ✅ Leverages existing rust-roon-api parsing and error handling
- ✅ More maintainable than Option A

**Disadvantages:**
- ❌ Requires maintaining a fork of rust-roon-api
- ❌ Must merge upstream updates manually
- ❌ Breaking API changes to Parsed enum affect all users
- ❌ Higher memory usage (multiple queues cached)
- ❌ Increased WebSocket traffic from Roon server
- ❌ Need to handle unsubscribe for individual zones

**When to consider:**
- Long-term need for multi-zone queue support
- Application architecture requires all queues to be current
- Willing to maintain library fork
- Team has bandwidth for ongoing maintenance
- Could contribute changes back to upstream if generally useful

---

### Comparison Matrix

| Feature | Option A (Raw) | Option B (Fork) | Option C (Current) |
|---------|---------------|-----------------|-------------------|
| Implementation complexity | High | Medium | Low |
| Maintenance burden | High | Medium | Low |
| Library modifications | None | Extensive | None |
| Multi-zone support | Full | Full | Single active |
| Memory usage | High | High | Low |
| WebSocket traffic | High | High | Low |
| Queue staleness | None | None | Inactive zones stale |
| Response time | Instant | Instant | Brief delay on switch |
| Code maintainability | Low | Medium | High |
| Risk of bugs | High | Medium | Low |

---

### Recommendations for Future

**Stick with Option C unless:**
1. User feedback indicates need for simultaneous multi-zone queue display
2. Application adds feature requiring real-time queue monitoring across zones
3. WebSocket traffic/memory concerns become irrelevant

**If upgrading becomes necessary:**
1. Start with **Option B** (fork modification) - cleaner than Option A
2. Consider contributing changes upstream to rust-roon-api
3. Could maintain Option C as fallback mode for resource-constrained scenarios

**Technical debt to track:**
- Option C limitation: stale queues for inactive zones
- Potential future requirement: multi-zone queue display dashboard
- Monitor upstream rust-roon-api for multi-subscription support

---

## Completed
- ✅ Version 1.0.0 - Initial release with queue display
- ✅ Version 1.1.0 - UI improvements and reconnect functionality
- ✅ Version 1.2.0 - Fullscreen support and responsive design
- ✅ Multi-zone queue support via on-demand subscription (Option C)
