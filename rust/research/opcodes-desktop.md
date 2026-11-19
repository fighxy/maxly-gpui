# Desktop opcode table (MAX 26.41.0.83760)

Extracted from C++ template names in `C:\MAX\core.dll` (`CommonPacket<opcode, Request, Response, flag>`, `CommonEvent<opcode, ..., Response>`) by `re/scripts/opcodes.sh`; table built by `re/scripts/opcodes_md.mjs`. PyMax names are hints only (ADR-002). "Not found" means not built with these templates (for example PING), not that the desktop lacks it. The meaning of the flag (0 for `SessionInit`, 2 for most) is still to be read in Ghidra.

| Opcode | Kind | Desktop type | Response type | Flag | PyMax name (hint) |
|---:|---|---|---|---|---|
| 5 | packet | Log | EmptyResponse | 2 | LOG |
| 6 | packet | SessionInit | SessionInit | 0 | SESSION_INIT |
| 16 | packet | Profile | Profile | 2 | PROFILE |
| 17 | packet | Auth::OneMe::AuthRequest | Auth::OneMe::AuthRequest | 0 | AUTH_REQUEST |
| 18 | packet | Auth::OneMe::Auth | Auth::OneMe::Auth | 0 | AUTH |
| 19 | packet | Auth::Login | Auth::Login | 0 | LOGIN |
| 20 | event, packet | LogoutData<br>EmptyParameters | Events::LogoutData<br>EmptyResponse | 2 | LOGOUT |
| 22 | packet | Config | Config | 2 | CONFIG |
| 23 | packet | Auth::OneMe::AuthConfirm | Auth::OneMe::AuthConfirm | 0 | AUTH_CONFIRM |
| 25 | packet | EmptyParameters | Avatar::PresetAvatar | 2 | PRESET_AVATARS |
| 26 | packet | Assets::GetAsset::Common | Assets::GetAsset::StickerSet | 2 | ASSETS_GET |
| 27 | packet | Assets::Update | Assets::Update | 2 | ASSETS_UPDATE |
| 28 | packet | Assets::GetByIds::Common | Assets::GetByIds::AnimojiSet | 2 | ASSETS_GET_BY_IDS |
| 29 | packet | Assets::Add | Assets::Add | 2 | ASSETS_ADD |
| 32 | packet | Contact::ContactInfo | Contact::ContactInfo | 2 | CONTACT_INFO |
| 34 | packet | Contact::ContactUpdate | Contact::ContactUpdate | 2 | CONTACT_UPDATE |
| 35 | packet | Contact::Presence | Contact::Presence | 2 | CONTACT_PRESENCE |
| 36 | packet | Contact::ContactList | Contact::ContactList | 2 | CONTACT_LIST |
| 39 | packet | Contact::ContactPhotos | Contact::ContactPhotos | 2 | CONTACT_PHOTOS |
| 41 | packet | Contact::ContactCreate | Contact::ContactCreate | 2 | — |
| 43 | packet | Contact::RemoveContactPhoto | Contact::RemoveContactPhoto | 2 | REMOVE_CONTACT_PHOTO |
| 46 | packet | Contact::ContactByPhone | Contact::ContactByPhone | 2 | CONTACT_INFO_BY_PHONE |
| 48 | packet | Chat::ChatInfo | Chat::ChatInfo | 2 | CHAT_INFO |
| 49 | packet | Chat::ChatHistory | Chat::ChatHistory | 2 | CHAT_HISTORY |
| 50 | packet | Chat::ChatMark | Chat::ChatMark | 2 | CHAT_MARK |
| 51 | packet | Chat::ChatMedia | Chat::ChatMedia | 2 | CHAT_MEDIA |
| 52 | packet | Chat::ChatDelete | EmptyResponse | 2 | CHAT_DELETE |
| 53 | packet | Chat::ChatList | Chat::ChatList | 2 | CHATS_LIST |
| 54 | packet | Chat::ChatClear | EmptyResponse | 2 | CHAT_CLEAR |
| 55 | packet | Chat::ChatUpdate | Chat::ChatUpdate | 2 | CHAT_UPDATE |
| 56 | packet | Chat::CheckLink | Chat::CheckLink | 2 | CHAT_CHECK_LINK |
| 57 | packet | Chat::Join | Chat::Join | 2 | CHAT_JOIN |
| 58 | packet | Chat::ChatLeave | Chat::ChatLeave | 2 | CHAT_LEAVE |
| 59 | packet | Chat::ChatMembers | Chat::ChatMembers | 2 | CHAT_MEMBERS |
| 60 | packet | Search::Public | Search::Public | 2 | PUBLIC_SEARCH |
| 61 | packet | Chat::PersonalConfig | Chat::PersonalConfig | 2 | CHAT_PERSONAL_CONFIG |
| 62 | packet | LiveStream::LiveStreamInfo | LiveStream::LiveStreamInfo | 2 | CHAT_LIVESTREAM_INFO |
| 64 | packet | Messaging::Send | Messaging::Send | 2 | MSG_SEND |
| 65 | packet | Typing::Send | EmptyResponse | 2 | MSG_TYPING |
| 66 | packet | Messaging::Delete | Messaging::Delete | 2 | MSG_DELETE |
| 67 | packet | Messaging::Edit | Messaging::Edit | 2 | MSG_EDIT |
| 68 | packet | Search::Chat | Search::Chat | 2 | CHAT_SEARCH |
| 69 | packet | Calls::VideoChatEdit | EmptyResponse | 2 | — |
| 71 | packet | Messaging::MsgGet | Messaging::MsgGet | 2 | MSG_GET |
| 72 | packet | Search::Message::Touch | EmptyResponse | 2 | MSG_SEARCH_TOUCH |
| 73 | packet | Search::Message | Search::Message | 2 | MSG_SEARCH |
| 74 | packet | Chat::MsgGetStat | Chat::MsgGetStat | 2 | MSG_GET_STAT |
| 75 | packet | Chat::ChatSubscribe | EmptyResponse | 2 | CHAT_SUBSCRIBE |
| 76 | packet | Calls::VideoChatStart | Calls::VideoChatStart | 2 | VIDEO_CHAT_START |
| 77 | packet | Chat::ChatMembersUpdate | Chat::ChatMembersUpdate | 2 | CHAT_MEMBERS_UPDATE |
| 78 | packet | Calls::VideoChatStartActive | Calls::VideoChatStartActive | 2 | VIDEO_CHAT_START_ACTIVE |
| 79 | packet | Calls::VideoChatHistory | Calls::VideoChatHistory | 2 | VIDEO_CHAT_HISTORY |
| 80 | packet | Attachments::PhotoUpload | Attachments::PhotoUpload | 2 | PHOTO_UPLOAD |
| 82 | packet | Attachments::VideoUpload | Attachments::VideoUpload | 2 | VIDEO_UPLOAD |
| 83 | packet | Attachments::VideoPlay | Attachments::VideoPlay | 2 | VIDEO_PLAY |
| 84 | packet | Calls::VideoChatCreateJoinLink | Calls::VideoChatCreateJoinLink | 2 | VIDEO_CHAT_CREATE_JOIN_LINK |
| 86 | packet | Chat::ChatPinSetVisibility | Chat::ChatPinSetVisibility | 2 | CHAT_PIN_SET_VISIBILITY |
| 87 | packet | Attachments::FileUpload | Attachments::FileUpload | 2 | FILE_UPLOAD |
| 88 | packet | Attachments::FileDownload | Attachments::FileDownload | 2 | FILE_DOWNLOAD |
| 89 | packet | LinkInfo | LinkInfo | 2 | LINK_INFO |
| 91 | packet | Comments::GetCommentsUpdates | Comments::GetCommentsUpdates | 2 | MSG_GET_COMMENTS_INFO |
| 94 | packet | Messaging::DeleteUserMessages | EmptyResponse | 2 | MSG_DELETE_USER_COMMENTS |
| 96 | packet | EmptyParameters | SessionsInfo | 2 | SESSIONS_INFO |
| 97 | packet | EmptyParameters | SessionsClose | 2 | SESSIONS_CLOSE |
| 101 | packet | Auth::OneMe::LoginRestorePassword | Auth::OneMe::LoginRestorePassword | 0 | AUTH_LOGIN_RESTORE_PASSWORD |
| 104 | packet | Auth::OneMe::Details2FA | Auth::OneMe::Details2FA | 1 | AUTH_2FA_DETAILS |
| 105 | packet | ExternalCallback | ExternalCallback | 2 | EXTERNAL_CALLBACK |
| 106 | packet | WebApp::WebAppRequestPhoneHash | WebApp::WebAppRequestPhoneHash | 2 | — |
| 107 | packet | Auth::OneMe::ValidatePassword | EmptyResponse | 0 | AUTH_VALIDATE_PASSWORD |
| 108 | packet | Auth::OneMe::ValidateHint | EmptyResponse | 0 | AUTH_VALIDATE_HINT |
| 109 | packet | Auth::OneMe::VerifyEmail | Auth::OneMe::VerifyEmail | 0 | AUTH_VERIFY_EMAIL |
| 110 | packet | Auth::OneMe::CheckEmail | Auth::OneMe::CheckEmail | 0 | AUTH_CHECK_EMAIL |
| 111 | packet | Auth::OneMe::Set2FA | EmptyResponse | 0 | AUTH_SET_2FA |
| 112 | packet | Auth::OneMe::CreateTrack | Auth::OneMe::CreateTrack | 0 | AUTH_CREATE_TRACK |
| 113 | packet | Auth::OneMe::CheckPassword | Auth::OneMe::CheckPassword | 1 | AUTH_CHECK_PASSWORD |
| 115 | packet | Auth::OneMe::LoginCheckPassword | Auth::OneMe::LoginCheckPassword | 0 | AUTH_LOGIN_CHECK_PASSWORD |
| 116 | packet | Auth::OneMe::LoginProfileDelete | Auth::OneMe::LoginProfileDelete | 0 | AUTH_LOGIN_PROFILE_DELETE |
| 118 | packet | Messaging::SendCallback | Messaging::SendCallback | 2 | MSG_SEND_CALLBACK |
| 119 | packet | Bot::Suspend | EmptyResponse | 2 | SUSPEND_BOT |
| 120 | event | NotificationCallAcceptedData | Events::NotificationCallAcceptedData |  | — |
| 121 | event | NotificationCallFinishData | Events::NotificationCallFinishData |  | — |
| 128 | event | NotificationMessageData | Events::NotificationMessageData |  | NOTIF_MESSAGE |
| 129 | event | TypingData | Events::TypingData |  | NOTIF_TYPING |
| 130 | event | NotificationMarkData | Events::NotificationMarkData |  | NOTIF_MARK |
| 131 | event | NotificationContactData | Events::NotificationContactData |  | NOTIF_CONTACT |
| 132 | event | PresenceNotificationMessageData | Events::PresenceNotificationMessageData |  | NOTIF_PRESENCE |
| 134 | event | ConfigChangeData | Events::ConfigChangeData |  | NOTIF_CONFIG |
| 135 | event | NotificationChatData | Events::NotificationChatData |  | NOTIF_CHAT |
| 136 | event | NotificationAttachData | Events::NotificationAttachData |  | NOTIF_ATTACH |
| 140 | event | MessagesDeleteRangeData | Events::MessagesDeleteRangeData |  | NOTIF_MSG_DELETE_RANGE |
| 142 | event | MessagesDeleteData | Events::MessagesDeleteData |  | NOTIF_MSG_DELETE |
| 144 | packet | Bot::ChatCommands | Bot::ChatCommands | 2 | CHAT_BOT_COMMANDS |
| 145 | packet | Bot::BotInfo | Bot::BotInfo | 2 | BOT_INFO |
| 154 | event | NotificationMessageDelayedData | Events::NotificationMessageDelayedData |  | NOTIF_MSG_DELAYED |
| 155 | event | ReactionsChangedData | Events::ReactionsChangedData |  | NOTIF_MSG_REACTIONS_CHANGED |
| 156 | event | YouReactedData | Events::YouReactedData |  | NOTIF_MSG_YOU_REACTED |
| 158 | packet | Calls::CallsToken | Calls::CallsToken | 2 | CALLS_TOKEN |
| 159 | event | NotificationProfiletData | Events::NotificationProfiletData |  | NOTIF_PROFILE |
| 160 | packet | WebApp::WebAppInitData | WebApp::WebAppInitData | 2 | WEB_APP_INIT_DATA |
| 161 | packet | Complain | EmptyResponse | 2 | COMPLAIN |
| 162 | packet | ComplainReasonsGet | ComplainReasonsGet | 2 | COMPLAIN_REASONS_GET |
| 163 | packet | Calls::History::Get | Calls::History::Get | 2 | — |
| 164 | packet | Calls::History::Clear | EmptyResponse | 2 | — |
| 165 | event | NotificationCallsHistoryData | Events::NotificationCallsHistoryData |  | — |
| 166 | packet | Calls::VideoChatJoin | Calls::VideoChatJoin | 2 | VIDEO_CHAT_JOIN |
| 178 | packet | Reactions::MsgReact | Reactions::MsgReact | 2 | MSG_REACTION |
| 179 | packet | Reactions::CancelReaction | Reactions::CancelReaction | 2 | MSG_CANCEL_REACTION |
| 180 | packet | Reactions::GetReactions | Reactions::GetReactions | 2 | MSG_GET_REACTIONS |
| 181 | packet | Reactions::GetDetailedReactions | Reactions::GetDetailedReactions | 2 | MSG_GET_DETAILED_REACTIONS |
| 198 | packet | Chat::CommonChatList | Chat::CommonChatList | 2 | CHAT_SEARCH_COMMON_PARTICIPANTS |
| 199 | packet | ProfileDelete | ProfileDelete | 2 | PROFILE_DELETE |
| 200 | packet | EmptyParameters | GetDeleteTamUserTime | 2 | PROFILE_DELETE_TIME |
| 202 | packet | TranscribeMedia | TranscribeMedia | 2 | TRANSCRIBE_MEDIA |
| 203 | packet | Attachments::PhotoUrlRefresh | Attachments::PhotoUrlRefresh | 2 | — |
| 204 | packet | Chat::GetExternalChatRestrictions | Chat::GetExternalChatRestrictions | 2 | — |
| 208 | packet | Stories::StoriesList | Stories::StoriesList | 2 | STORIES_LIST |
| 209 | packet | Stories::StoriesListByOwnerId | Stories::StoriesListByOwnerId | 2 | STORIES_LIST_BY_OWNER_ID |
| 210 | packet | Stories::StoriesGetByOwnerId | Stories::StoriesGetByOwnerId | 2 | STORIES_GET_BY_OWNER_ID |
| 211 | packet | Stories::StoriesGetStats | Stories::StoriesGetStats | 2 | STORIES_GET_STATS |
| 212 | packet | Stories::StoriesGetDetailedStats | Stories::StoriesGetDetailedStats | 2 | STORIES_GET_DETAILED_STATS |
| 213 | packet | Stories::StoriesReact | Stories::StoriesReact | 2 | STORIES_REACT |
| 214 | packet | Stories::StoriesMark | Stories::StoriesMark | 2 | STORIES_MARK |
| 216 | event | NotificationStoriesUpdateData | Events::NotificationStoriesUpdateData |  | NOTIF_STORIES_UPDATE |
| 217 | packet | Stories::StoriesEdit | Stories::StoriesEdit | 2 | STORIES_EDIT |
| 218 | packet | Stories::StoriesDelete | Stories::StoriesDelete | 2 | STORIES_DELETE |
| 219 | packet | Stories::StoriesHistoryGetByOwnerId | Stories::StoriesHistoryGetByOwnerId | 2 | — |
| 220 | packet | Stories::StoriesGetByStoryId | Stories::StoriesGetByStoryId | 2 | STORIES_GET_BY_STORY_ID |
| 221 | packet | Chat::ChatLinksList | Chat::ChatLinksList | 2 | — |
| 222 | packet | Chat::ChatLinkCreate | Chat::ChatLinkCreate | 2 | — |
| 223 | packet | Chat::ChatLinkDelete | Chat::ChatLinkDelete | 2 | — |
| 240 | packet | PinnedMessages::GetStates | PinnedMessages::GetStates | 2 | — |
| 241 | packet | PinnedMessages::Get | PinnedMessages::Get | 2 | — |
| 242 | packet | PinnedMessages::Update | PinnedMessages::Update | 2 | — |
| 243 | event | NotificationChatMessagePinnedData | Events::NotificationChatMessagePinnedData |  | — |
| 256 | packet | OrgInfo | OrgInfo | 2 | ORG_INFO |
| 257 | packet | Reactions::SetChatReactions | Reactions::SetChatReactions | 2 | CHAT_REACTIONS_SETTINGS_SET |
| 258 | packet | Reactions::GetChatReactions | Reactions::GetChatReactions | 2 | REACTIONS_SETTINGS_GET_BY_CHAT_ID |
| 259 | packet | Assets::Remove | Assets::Remove | 2 | ASSETS_REMOVE |
| 261 | packet | Assets::ListModify | Assets::ListModify | 2 | ASSETS_LIST_MODIFY |
| 272 | packet | Folders::Get | Folders::Get | 2 | FOLDERS_GET |
| 274 | packet | Folders::Update | Folders::Update | 2 | FOLDERS_UPDATE |
| 275 | packet | Folders::Reorder | Folders::Reorder | 2 | FOLDERS_REORDER |
| 276 | packet | Folders::Delete | Folders::Delete | 2 | FOLDERS_DELETE |
| 277 | event | NotificationFoldersData | Events::NotificationFoldersData |  | NOTIF_FOLDERS |
| 288 | packet | EmptyParameters | Auth::OneMe::CreateQr | 0 | GET_QR |
| 289 | packet | Auth::OneMe::AuthCheck | Auth::OneMe::AuthCheck | 0 | GET_QR_STATUS |
| 291 | packet | Auth::OneMe::AuthByTrack | Auth::OneMe::AuthByTrack | 0 | LOGIN_BY_QR |
| 293 | event | NotificationTranscriptionData | Events::NotificationTranscriptionData |  | NOTIF_TRANSCRIPTION |
| 294 | packet | Auth::OneMe::CheckCall | Auth::OneMe::CheckCall | 0 | — |
| 300 | packet | Suggest::ChatSuggest | Suggest::ChatSuggest | 2 | CHAT_SUGGEST |
| 301 | packet | Attachments::AudioPlay | Attachments::AudioPlay | 2 | AUDIO_PLAY |
| 302 | packet | Banners::BannersGet | Banners::BannersGet | 2 | BANNERS_GET |
| 304 | packet | Polls::SendVote | Polls::SendVote | 2 | SEND_VOTE |
| 305 | packet | Polls::VotersListByAnswer | Polls::VotersListByAnswer | 2 | VOTERS_LIST_BY_ANSWER |
| 306 | packet | Polls::GetPollUpdates | Polls::GetPollUpdates | 2 | GET_POLL_UPDATES |
| 307 | packet | EmptyParameters | EmptyResponse | 2 | — |
| 308 | packet | EntityOrgInfo | EntityOrgInfo | 2 | — |

157 desktop opcodes. In PyMax but not found as CommonPacket/CommonEvent in the desktop binary (43):

`1 PING`, `2 DEBUG`, `3 RECONNECT`, `8 LOGIN2`, `21 SYNC`, `31 SEARCH_FEEDBACK`, `33 CONTACT_ADD`, `37 CONTACT_SEARCH`, `38 CONTACT_MUTUAL`, `40 CONTACT_SORT`, `42 CONTACT_VERIFY`, `63 CHAT_CREATE`, `70 MSG_SHARE_PREVIEW`, `81 STICKER_UPLOAD`, `92 MSG_DELETE_RANGE`, `98 PHONE_BIND_REQUEST`, `99 PHONE_BIND_CONFIRM`, `103 GET_INBOUND_CALLS`, `117 CHAT_COMPLAIN`, `124 LOCATION_STOP`, `125 LOCATION_SEND`, `126 LOCATION_REQUEST`, `127 GET_LAST_MENTIONS`, `137 NOTIF_CALL_START`, `139 NOTIF_CONTACT_SORT`, `143 NOTIF_CALLBACK_ANSWER`, `147 NOTIF_LOCATION`, `148 NOTIF_LOCATION_REQUEST`, `150 NOTIF_ASSETS_UPDATE`, `152 NOTIF_DRAFT`, `153 NOTIF_DRAFT_DISCARD`, `176 DRAFT_SAVE`, `177 DRAFT_DISCARD`, `193 STICKER_CREATE`, `194 STICKER_SUGGEST`, `195 VIDEO_CHAT_MEMBERS`, `196 CHAT_HIDE`, `215 STORIES_SEND`, `260 ASSETS_MOVE`, `273 FOLDERS_GET_BY_ID`, `290 AUTH_QR_APPROVE`, `292 NOTIF_BANNERS`, `303 MSG_DELIVERY`

Desktop-only opcodes (not in PyMax, 21): 41, 69, 106, 120, 121, 163, 164, 165, 203, 204, 219, 221, 222, 223, 240, 241, 242, 243, 294, 307, 308
