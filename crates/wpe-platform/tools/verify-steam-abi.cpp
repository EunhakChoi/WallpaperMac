// Compile against the user's official SDK public/steam headers before deploying
// this pinned v020 flat profile. This file contains no SDK implementation.
#include <cstddef>
#include <type_traits>
#include "steam_api.h"
#include "steam_api_flat.h"
static_assert(sizeof(void*) == 8);
static_assert(offsetof(CallbackMsg_t, m_pubParam) == 8);
static_assert(offsetof(CallbackMsg_t, m_cubParam) == 16);
static_assert(SteamAPICallCompleted_t::k_iCallback == 703);
static_assert(SteamUGCQueryCompleted_t::k_iCallback == 3401);
static_assert(CreateItemResult_t::k_iCallback == 3403);
static_assert(SubmitItemUpdateResult_t::k_iCallback == 3404);
static_assert(DownloadItemResult_t::k_iCallback == 3406);
static_assert(RemoteStorageSubscribePublishedFileResult_t::k_iCallback == 1313);
static_assert(RemoteStorageUnsubscribePublishedFileResult_t::k_iCallback == 1315);
static_assert(offsetof(DownloadItemResult_t, m_nPublishedFileId) == 4);
static_assert(offsetof(DownloadItemResult_t, m_eResult) == 12);
static_assert(offsetof(CreateItemResult_t, m_nPublishedFileId) == 4);
static_assert(offsetof(CreateItemResult_t, m_bUserNeedsToAcceptWorkshopLegalAgreement) == 12);
static_assert(offsetof(SteamUGCDetails_t, m_rgchTitle) == 24);
static_assert(offsetof(SteamUGCDetails_t, m_rgchDescription) == 153);
static_assert(sizeof(SteamUGCDetails_t) <= 32768);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamUGC_GetNumSubscribedItems), uint32 (*)(ISteamUGC*)>);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamUGC_GetSubscribedItems), uint32 (*)(ISteamUGC*,PublishedFileId_t*,uint32)>);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamApps_GetDLCCount),int (*)(ISteamApps*)>);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamApps_BIsDlcInstalled),bool (*)(ISteamApps*,AppId_t)>);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamApps_GetDlcDownloadProgress),bool (*)(ISteamApps*,AppId_t,uint64*,uint64*)>);
static_assert(DeleteItemResult_t::k_iCallback == 3417);
static_assert(offsetof(DeleteItemResult_t, m_nPublishedFileId)==4);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamUGC_DeleteItem), SteamAPICall_t (*)(ISteamUGC*,PublishedFileId_t)>);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamUser_GetSteamID), uint64 (*)(ISteamUser*)>);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamUGC_CreateQueryUserUGCRequest), UGCQueryHandle_t (*)(ISteamUGC*,AccountID_t,EUserUGCList,EUGCMatchingUGCType,EUserUGCListSortOrder,AppId_t,AppId_t,uint32)>);
static_assert(UserStatsReceived_t::k_iCallback == 1101);
static_assert(UserStatsStored_t::k_iCallback == 1102);
static_assert(offsetof(UserStatsReceived_t,m_steamIDUser)==12);
static_assert(offsetof(UserStatsStored_t,m_eResult)==8);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamUserStats_SetAchievement),bool (*)(ISteamUserStats*,const char*)>);
static_assert(std::is_same_v<decltype(&SteamAPI_ISteamUserStats_RequestCurrentStats),bool (*)(ISteamUserStats*)>);
int main() { return 0; }
