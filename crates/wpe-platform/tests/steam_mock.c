// Owned ABI exercise fixture, not a Steamworks SDK implementation or service.
#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#include <stdio.h>
static int mode,shutdowns,releases,frees,last_tags,achievement_sets,stats_stores;static bool stats_received,stats_stored;static uint64_t serial=100,next_call,item=99;static int callback_type;static bool queued,download_queue;
typedef struct {int user;int callback;unsigned char*data;int bytes;} CallbackMsg;
#pragma pack(push,4)
typedef struct{uint64_t call;int callback;uint32_t size;} Completed;
typedef struct{uint64_t query;int result;uint32_t returned,total;bool cached;unsigned char padding[3];} Query;
typedef struct{uint32_t app;uint64_t item;int result;} Download;
typedef struct{uint64_t game;int result;uint64_t user;} Stats;
typedef struct{uint64_t game;int result;} Stored;
#pragma pack(pop)
static Completed completed;static Query query;static Download download;static Stats stats;static Stored stored;static unsigned char payload[64];
void MockReset(int m){mode=m;shutdowns=releases=frees=last_tags=0;queued=download_queue=stats_received=stats_stored=false;achievement_sets=stats_stores=0;}
int MockCounter(int n){return n==0?shutdowns:n==1?releases:n==2?frees:n==4?achievement_sets:n==5?stats_stores:last_tags;}
bool SteamAPI_Init(void){return mode!=2;}void SteamAPI_Shutdown(void){shutdowns++;}
int SteamAPI_GetHSteamPipe(void){return 1;}void SteamAPI_ManualDispatch_Init(void){}void SteamAPI_ManualDispatch_RunFrame(int pipe){(void)pipe;}
void*SteamAPI_SteamUGC_v020(void){return(void*)1;}void*SteamAPI_SteamUtils_v010(void){return(void*)2;}
uint32_t SteamAPI_ISteamUtils_GetAppID(void*self){(void)self;return 431960;}
static uint64_t enqueue(int cb,uint64_t id){next_call=++serial;callback_type=cb;item=id;queued=true;return next_call;}
bool SteamAPI_ManualDispatch_GetNextCallback(int pipe,CallbackMsg*out){(void)pipe;if(mode==1)return false;if(queued){queued=false;completed=(Completed){next_call,mode==5?3403:callback_type,callback_type==3401?sizeof(Query):callback_type==3403?13:callback_type==3404?5:12};*out=(CallbackMsg){1,703,(unsigned char*)&completed,mode==3?-1:(int)sizeof(Completed)};return true;}if(stats_received){stats_received=false;stats=(Stats){mode==14?1:431960,1,mode==13?1:UINT64_C(76561197960265827)};*out=(CallbackMsg){1,1101,(unsigned char*)&stats,sizeof(stats)};return true;}if(stats_stored){stats_stored=false;stored=(Stored){431960,mode==12?42:1};*out=(CallbackMsg){1,1102,(unsigned char*)&stored,sizeof(stored)};return true;}if(download_queue){download_queue=false;download=(Download){mode==6?1:431960,item,1};*out=(CallbackMsg){1,3406,(unsigned char*)&download,sizeof(download)};return true;}return false;}
void SteamAPI_ManualDispatch_FreeLastCallback(int pipe){(void)pipe;frees++;}
bool SteamAPI_ManualDispatch_GetAPICallResult(int pipe,uint64_t call,void*out,int size,int cb,bool*failed){(void)pipe;(void)call;*failed=mode==4;if(*failed)return true;memset(payload,0,sizeof(payload));int result=mode==8?42:1;memcpy(payload,&result,4);if(cb==3401){query=(Query){7,result,1,1,false,{0}};memcpy(out,&query,size);return true;}if(cb==3403){uint64_t created=123456;memcpy(payload+4,&created,8);payload[12]=1;}else if(cb==3404){payload[4]=1;}else{memcpy(payload+4,&item,8);}memcpy(out,payload,size);return true;}
uint64_t SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest(void*self,uint64_t*ids,uint32_t n){(void)self;(void)ids;(void)n;return 7;}
uint64_t SteamAPI_ISteamUGC_CreateQueryAllUGCRequestPage(void*self,int type,int match,uint32_t creator,uint32_t consumer,uint32_t page){(void)self;(void)type;(void)match;(void)creator;(void)consumer;(void)page;return 7;}
bool SteamAPI_ISteamUGC_SetReturnLongDescription(void*self,uint64_t q,bool enabled){(void)self;(void)q;return enabled;}
uint64_t SteamAPI_ISteamUGC_SendQueryUGCRequest(void*self,uint64_t q){(void)self;(void)q;return enqueue(3401,99);}
bool SteamAPI_ISteamUGC_ReleaseQueryUGCRequest(void*self,uint64_t q){(void)self;(void)q;releases++;return true;}
bool SteamAPI_ISteamUGC_GetQueryUGCResult(void*self,uint64_t q,uint32_t index,void*out){(void)self;(void)q;(void)index;memset(out,0,32768);uint64_t id=99;int result=1;uint32_t app=mode==7?1:431960;memcpy(out,&id,8);memcpy((char*)out+8,&result,4);memcpy((char*)out+16,&app,4);memcpy((char*)out+20,&app,4);strcpy((char*)out+24,"Mock title");strcpy((char*)out+153,"Mock description");return true;}
uint32_t SteamAPI_ISteamUGC_GetNumSubscribedItems(void*self){(void)self;return 1;}
uint32_t SteamAPI_ISteamUGC_GetSubscribedItems(void*self,uint64_t*out,uint32_t count){(void)self;if(count)out[0]=99;return count?1:0;}
uint32_t SteamAPI_ISteamUGC_GetItemState(void*self,uint64_t id){(void)self;(void)id;return 5;}
bool SteamAPI_ISteamUGC_GetItemInstallInfo(void*self,uint64_t id,uint64_t*bytes,char*folder,uint32_t capacity,uint32_t*time){(void)self;(void)id;*bytes=123;*time=1;snprintf(folder,capacity,"/owned/mock/99");return true;}
bool SteamAPI_ISteamUGC_DownloadItem(void*self,uint64_t id,bool high){(void)self;(void)high;item=id;download_queue=true;return true;}
bool SteamAPI_ISteamUGC_GetItemDownloadInfo(void*self,uint64_t id,uint64_t*done,uint64_t*total){(void)self;(void)id;*done=20;*total=100;return true;}
uint64_t SteamAPI_ISteamUGC_SubscribeItem(void*self,uint64_t id){(void)self;return enqueue(1313,id);}
uint64_t SteamAPI_ISteamUGC_UnsubscribeItem(void*self,uint64_t id){(void)self;return enqueue(1315,id);}
uint64_t SteamAPI_ISteamUGC_CreateItem(void*self,uint32_t app,int type){(void)self;(void)app;(void)type;return enqueue(3403,0);}
uint64_t SteamAPI_ISteamUGC_StartItemUpdate(void*self,uint32_t app,uint64_t id){(void)self;(void)app;item=id;return 88;}
bool SteamAPI_ISteamUGC_SetItemContent(void*self,uint64_t handle,const char*text){(void)self;(void)handle;return text&&*text;}
bool SteamAPI_ISteamUGC_SetItemPreview(void*self,uint64_t handle,const char*text){(void)self;(void)handle;return text&&*text;}
bool SteamAPI_ISteamUGC_SetItemTitle(void*self,uint64_t handle,const char*text){(void)self;(void)handle;return text&&*text;}
bool SteamAPI_ISteamUGC_SetItemDescription(void*self,uint64_t handle,const char*text){(void)self;(void)handle;return text!=0;}
typedef struct{const char*const*strings;int count;} StringArray;
bool SteamAPI_ISteamUGC_SetItemTags(void*self,uint64_t handle,const StringArray*tags){(void)self;(void)handle;last_tags=tags->count;return tags->count==0||tags->strings[0][0];}
bool SteamAPI_ISteamUGC_SetItemVisibility(void*self,uint64_t handle,int visibility){(void)self;(void)handle;return visibility>=0&&visibility<=3;}
uint64_t SteamAPI_ISteamUGC_SubmitItemUpdate(void*self,uint64_t handle,const char*note){(void)self;(void)handle;(void)note;return enqueue(3404,item);}
int SteamAPI_ISteamUGC_GetItemUpdateProgress(void*self,uint64_t handle,uint64_t*done,uint64_t*total){(void)self;(void)handle;*done=50;*total=100;return 4;}

static bool dlc_installed=false;
void*SteamAPI_SteamApps_v008(void){return mode==9?0:(void*)3;}
int SteamAPI_ISteamApps_GetDLCCount(void*self){(void)self;return 1;}
bool SteamAPI_ISteamApps_BGetDLCDataByIndex(void*self,int index,uint32_t*id,bool*available,char*name,int bytes){(void)self;if(index!=0)return false;*id=123;*available=true;snprintf(name,(size_t)bytes,"Owned mock DLC");return true;}
bool SteamAPI_ISteamApps_BIsDlcInstalled(void*self,uint32_t id){(void)self;return id==123&&dlc_installed;}
void SteamAPI_ISteamApps_InstallDLC(void*self,uint32_t id){(void)self;if(id==123)dlc_installed=true;}
void SteamAPI_ISteamApps_UninstallDLC(void*self,uint32_t id){(void)self;if(id==123)dlc_installed=false;}
bool SteamAPI_ISteamApps_GetDlcDownloadProgress(void*self,uint32_t id,uint64_t*done,uint64_t*total){(void)self;if(id!=123)return false;*done=30;*total=100;return true;}

void*SteamAPI_SteamUser_v023(void){return(void*)4;}
uint64_t SteamAPI_ISteamUser_GetSteamID(void*self){(void)self;return UINT64_C(76561197960265827);}
uint64_t SteamAPI_ISteamUGC_CreateQueryUserUGCRequest(void*self,uint32_t account,int list,int match,int sort,uint32_t creator,uint32_t consumer,uint32_t page){(void)self;return account==99&&list==0&&match==-1&&sort==0&&creator==431960&&consumer==431960&&page==1?7:UINT64_MAX;}
uint64_t SteamAPI_ISteamUGC_DeleteItem(void*self,uint64_t id){(void)self;return enqueue(3417,id);}
bool SteamAPI_ISteamUGC_SetSearchText(void*self,uint64_t q,const char*text){(void)self;return q==7&&text&&*text;}
bool SteamAPI_ISteamUGC_AddRequiredTag(void*self,uint64_t q,const char*text){(void)self;return q==7&&text&&*text;}
bool SteamAPI_ISteamUGC_AddExcludedTag(void*self,uint64_t q,const char*text){(void)self;return q==7&&text&&*text;}

void*SteamAPI_SteamUserStats_v012(void){return mode==15?0:(void*)5;}
bool SteamAPI_ISteamUserStats_RequestCurrentStats(void*self){(void)self;stats_received=true;return true;}
uint32_t SteamAPI_ISteamUserStats_GetNumAchievements(void*self){(void)self;return mode==11?0:4;}
const char*SteamAPI_ISteamUserStats_GetAchievementName(void*self,uint32_t index){(void)self;static const char*names[]={"POST_SCENE_WALLPAPER","POST_WEB_WALLPAPER","POST_APP_WALLPAPER","POST_VIDEO_WALLPAPER"};return index<4?names[index]:"";}
bool SteamAPI_ISteamUserStats_SetAchievement(void*self,const char*name){(void)self;if(strncmp(name,"POST_",5))return false;achievement_sets++;return true;}
bool SteamAPI_ISteamUserStats_StoreStats(void*self){(void)self;stats_stores++;stats_stored=true;return true;}
